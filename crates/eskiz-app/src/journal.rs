//! Crash-recovery journal: a small, frequently written record of unsaved work.
//!
//! While a document is dirty, the UI thread plans a snapshot at most every
//! [`JOURNAL_SECS`] seconds, and once a stroke has been committed (debounced by
//! [`STROKE_DEBOUNCE_SECS`]). A plan compares the current layers with the previous plan
//! tile by tile. Tiles are copy-on-write `Arc`s, so "changed" is a pointer comparison and
//! costs nothing to find. Only the changed tiles are sent to the storage worker, which
//! writes them as tile files, then commits a new `journal.json` manifest atomically.
//!
//! On disk, `<recovery>/<id>/` holds:
//!
//! * `journal.json`: the manifest. It has the layer tree (ids, names, flags), the canvas
//!   size and depth, `base_keys` (the layer ids of the last `.procreate` save, in the order the
//!   package stores them, or `null` when the journal holds every pixel by itself), and the list
//!   of tile records `{layer, tx, ty, file}` (`file: null` means the tile is empty).
//! * `<seq>-<layer>-<tx>-<ty>.tile`: one changed tile, a 5-byte header then raw pixels.
//!
//! The manifest is the commit point. Tile files that it does not list are garbage and are
//! removed after it is written. A successful save of the package removes the directory.
//!
//! Restore rebuilds the document from the saved package (for the layers in `base_keys`),
//! overlays the journal's tiles, and keeps the ids, so the layers come back pixel for pixel.
//! Layer and group masks are journaled as whole files (`<seq>-mask-<id>.tile`, rewritten only
//! when the mask changed). Text sources are not journaled: they come from the save.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use eskiz_core::canvas::{Layer, Tile};
use eskiz_core::document::{Node, NodeKind, Props};
use eskiz_core::{BlendMode, ColourSpace, Depth};
use serde_json::{json, Value};

use crate::document::Document;
use crate::storage::{blend_index, write_atomic, Library};

/// Longest time between two periodic snapshots while the document is dirty, seconds.
pub const JOURNAL_SECS: f64 = 10.0;
/// Quiet time after a committed stroke before its snapshot is taken, seconds.
pub const STROKE_DEBOUNCE_SECS: f64 = 1.0;

const FORMAT: u64 = 1;
const MANIFEST: &str = "journal.json";
const TILE_EXT: &str = "tile";
const TILE_MAGIC: &[u8; 4] = b"EJT1";
const MASK_MAGIC: &[u8; 4] = b"EJM1";

/// A snapshot the UI has planned and the worker has to write.
#[derive(Debug)]
pub struct Plan {
    pub id: String,
    /// The document's directory in the recovery folder.
    pub dir: PathBuf,
    /// The plan rebuilds the journal from its base, discarding earlier records.
    pub reset: bool,
    /// Changed tiles to write: file name and pixels.
    pub tiles: Vec<(String, Arc<Tile>)>,
    /// Changed masks to write: file name and the mask's bytes.
    pub masks: Vec<(String, Arc<Vec<u8>>)>,
    /// The manifest, complete for this snapshot.
    pub manifest: Vec<u8>,
    /// Names of the tile files the manifest lists. Other `.tile` files in `dir` are removed.
    pub keep: HashSet<String>,
}

/// A journal found at start-up that is newer than the package it belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pending {
    pub id: String,
    pub title: String,
    /// When the snapshot was written.
    pub saved_at: SystemTime,
}

/// Layer pixels by layer id, as they were at one snapshot.
type Pixels = Vec<(u64, Layer)>;

/// The package the journal is relative to.
#[derive(Clone)]
struct Base {
    keys: Vec<u64>,
    layers: Pixels,
    depth: Depth,
    size: (usize, usize),
}

/// The UI-side planner for one document. It remembers the previous snapshot, so each plan
/// writes only what changed.
#[derive(Clone)]
pub struct Planner {
    base: Option<Base>,
    /// Masks at the last plan, by node id: the file that holds each and its bytes.
    masks: BTreeMap<u64, (String, Arc<Vec<u8>>)>,
    /// Layers at the last plan.
    prev: Pixels,
    /// Tile records relative to `base`: `(layer, tx, ty)` to file name, `None` when empty.
    records: BTreeMap<(u64, usize, usize), Option<String>>,
    seq: u64,
    /// Set when a journal write failed. The next plan rebuilds the journal from its base.
    broken: bool,
}

impl Planner {
    /// A planner for a document with no package yet: the journal holds every pixel.
    pub fn unsaved() -> Self {
        Self { base: None, masks: BTreeMap::new(), prev: Vec::new(), records: BTreeMap::new(), seq: 0, broken: false }
    }

    /// A planner whose base is the package just written or opened from `doc`.
    pub fn based_on(doc: &Document) -> Self {
        let mut p = Self::unsaved();
        p.rebase(doc);
        p
    }

    /// The package now holds `doc`: the journal starts again from it.
    pub fn rebase(&mut self, doc: &Document) {
        let pixels = pixels_of(doc);
        self.base = Some(Base {
            keys: doc.layer_ids(),
            layers: pixels.clone(),
            depth: doc.depth,
            size: (doc.width, doc.height),
        });
        self.prev = pixels;
        self.records.clear();
        self.masks.clear();
        self.broken = false;
    }

    /// A journal write failed: the next plan starts again from the base.
    pub fn mark_broken(&mut self) {
        self.broken = true;
    }

    /// Plan the snapshot of `doc`: which tiles changed since the last plan, and the manifest.
    pub fn plan(&mut self, doc: &Document, root: &Path, id: &str, title: &str) -> Plan {
        // A depth or canvas size change makes the base useless (its tiles are of another depth
        // or grid), so the journal then holds every pixel by itself.
        let base_changed = self
            .base
            .as_ref()
            .is_some_and(|b| b.depth != doc.depth || b.size != (doc.width, doc.height));
        if base_changed {
            self.base = None;
        }
        let reset = self.broken || base_changed;
        self.broken = false;
        let reference: Pixels = if reset {
            self.records.clear();
            self.masks.clear();
            self.base.as_ref().map(|b| b.layers.clone()).unwrap_or_default()
        } else {
            self.prev.clone()
        };
        let reference: HashMap<u64, Layer> = reference.into_iter().collect();
        self.seq += 1;
        let (w, h) = (doc.width, doc.height);
        let mut tiles = Vec::new();
        for key in doc.layer_ids() {
            let Some(layer) = doc.layer(key) else { continue };
            let blank = Layer::new_depth(w, h, doc.depth);
            let before = reference.get(&key).unwrap_or(&blank);
            for ty in 0..layer.tiles_y() {
                for tx in 0..layer.tiles_x() {
                    if before.tile_shared(layer, tx, ty) {
                        continue;
                    }
                    match layer.tile(tx, ty) {
                        Some(tile) => {
                            let name = format!("{}-{key}-{tx}-{ty}.{TILE_EXT}", self.seq);
                            tiles.push((name.clone(), Arc::clone(tile)));
                            self.records.insert((key, tx, ty), Some(name));
                        }
                        None => {
                            self.records.insert((key, tx, ty), None);
                        }
                    }
                }
            }
        }
        let ids: HashSet<u64> = doc.layer_ids().into_iter().collect();
        self.records.retain(|(key, _, _), _| ids.contains(key));
        self.prev = pixels_of(doc);

        // Masks: a changed or new mask gets a new file; the others keep theirs.
        let mut current = Vec::new();
        masks_of(&doc.root, &mut current);
        let mut masks = Vec::new();
        let mut next = BTreeMap::new();
        for (node, data) in current {
            let entry = match self.masks.remove(&node) {
                Some((name, old)) if old.as_slice() == data => (name, old),
                _ => {
                    let name = format!("{}-mask-{node}.{TILE_EXT}", self.seq);
                    let bytes = Arc::new(data.to_vec());
                    masks.push((name.clone(), Arc::clone(&bytes)));
                    (name, bytes)
                }
            };
            next.insert(node, entry);
        }
        self.masks = next;

        let mut keep: HashSet<String> = self.records.values().flatten().cloned().collect();
        keep.extend(self.masks.values().map(|(name, _)| name.clone()));
        let manifest = manifest_json(self, doc, id, title);
        Plan {
            id: id.to_owned(),
            dir: root.join(id),
            reset,
            tiles,
            masks,
            manifest: manifest.to_string().into_bytes(),
            keep,
        }
    }
}

fn manifest_json(planner: &Planner, doc: &Document, id: &str, title: &str) -> Value {
    let base_keys = match &planner.base {
        Some(b) => json!(b.keys),
        None => Value::Null,
    };
    let records: Vec<Value> = planner
        .records
        .iter()
        .map(|((layer, tx, ty), file)| json!({"layer": layer, "tx": tx, "ty": ty, "file": file}))
        .collect();
    json!({
        "format": FORMAT,
        "id": id,
        "title": title,
        "saved_at_ms": now_ms(),
        "seq": planner.seq,
        "width": doc.width,
        "height": doc.height,
        "depth": doc.depth.bits(),
        "dpi": doc.dpi,
        "background": doc.background,
        "background_visible": doc.background_visible,
        "colour_space": doc.colour_space.profile_name(),
        "base_keys": base_keys,
        "tree": tree_json(&doc.root),
        "records": records,
        "masks": planner.masks.iter().map(|(node, (file, _))| json!({"node": node, "file": file})).collect::<Vec<_>>(),
    })
}

/// Every layer and group mask in the tree, by node id.
fn masks_of<'a>(nodes: &'a [Node], out: &mut Vec<(u64, &'a [u8])>) {
    for n in nodes {
        if let Some(mask) = &n.mask {
            out.push((n.id, &mask.data));
        }
        if let NodeKind::Group { children, .. } = &n.kind {
            masks_of(children, out);
        }
    }
}

fn pixels_of(doc: &Document) -> Pixels {
    doc.layer_ids().into_iter().filter_map(|id| doc.layer(id).map(|l| (id, l.clone()))).collect()
}

fn tree_json(nodes: &[Node]) -> Value {
    Value::Array(nodes.iter().map(node_json).collect())
}

fn node_json(n: &Node) -> Value {
    let p = &n.props;
    let mut v = json!({
        "id": n.id,
        "name": p.name,
        "visible": p.visible,
        "opacity": p.opacity,
        "blend": blend_index(p.blend),
        "clipped": p.clipped,
        "alpha_lock": p.alpha_lock,
        "locked": p.locked,
        "private": p.private,
        "hold": p.hold,
        "anim_background": p.anim_background,
        "anim_foreground": p.anim_foreground,
    });
    match &n.kind {
        NodeKind::Layer => v["group"] = json!(false),
        NodeKind::Group { children, collapsed } => {
            v["group"] = json!(true);
            v["collapsed"] = json!(collapsed);
            v["children"] = tree_json(children);
        }
    }
    v
}

fn tree_from_json(v: &Value) -> Result<Vec<Node>, String> {
    v.as_array().ok_or("journal: bad layer tree")?.iter().map(node_from_json).collect()
}

fn node_from_json(v: &Value) -> Result<Node, String> {
    let id = v["id"].as_u64().ok_or("journal: bad node id")?;
    let mut props = Props::new(v["name"].as_str().unwrap_or("Layer"));
    props.visible = v["visible"].as_bool().unwrap_or(true);
    props.opacity = v["opacity"].as_f64().unwrap_or(1.0);
    props.blend = v["blend"].as_i64().and_then(BlendMode::from_index).unwrap_or(BlendMode::Normal);
    props.clipped = v["clipped"].as_bool().unwrap_or(false);
    props.alpha_lock = v["alpha_lock"].as_bool().unwrap_or(false);
    props.locked = v["locked"].as_bool().unwrap_or(false);
    props.private = v["private"].as_bool().unwrap_or(false);
    props.hold = v["hold"].as_u64().unwrap_or(0) as u32;
    props.anim_background = v["anim_background"].as_bool().unwrap_or(false);
    props.anim_foreground = v["anim_foreground"].as_bool().unwrap_or(false);
    let kind = if v["group"].as_bool().unwrap_or(false) {
        NodeKind::Group {
            children: tree_from_json(&v["children"])?,
            collapsed: v["collapsed"].as_bool().unwrap_or(false),
        }
    } else {
        NodeKind::Layer
    };
    Ok(Node { id, props, mask: None, kind })
}

fn max_id(nodes: &[Node]) -> u64 {
    nodes
        .iter()
        .map(|n| match &n.kind {
            NodeKind::Layer => n.id,
            NodeKind::Group { children, .. } => n.id.max(max_id(children)),
        })
        .max()
        .unwrap_or(0)
}

// ---------------------------------------------------------------- writing (worker thread)

/// Write a planned snapshot: the changed tiles, then the manifest, then remove the tile files
/// the manifest no longer lists.
pub fn execute(plan: &Plan) -> Result<(), String> {
    let _writing = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    execute_locked(plan)
}

/// Journal writes (the worker's and a panic flush) never run at the same time.
static WRITE_LOCK: Mutex<()> = Mutex::new(());

fn execute_locked(plan: &Plan) -> Result<(), String> {
    fs::create_dir_all(&plan.dir).map_err(|e| format!("{}: {e}", plan.dir.display()))?;
    for (name, tile) in &plan.tiles {
        crate::storage::write_atomic_nodir(&plan.dir.join(name), &encode_tile(tile))?;
    }
    for (name, mask) in &plan.masks {
        let mut bytes = Vec::with_capacity(4 + mask.len());
        bytes.extend_from_slice(MASK_MAGIC);
        bytes.extend_from_slice(mask);
        crate::storage::write_atomic_nodir(&plan.dir.join(name), &bytes)?;
    }
    // The tiles and the manifest's name must be on disk before the files it no longer lists go.
    crate::storage::sync_dir(&plan.dir);
    write_atomic(&plan.dir.join(MANIFEST), &plan.manifest)?;
    crate::storage::sync_dir(&plan.dir);
    if let Ok(entries) = fs::read_dir(&plan.dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_tile = path.extension().and_then(|e| e.to_str()) == Some(TILE_EXT);
            if is_tile && !plan.keep.contains(&name) {
                let _ = fs::remove_file(&path);
            }
        }
    }
    Ok(())
}

/// Write a full journal of `doc` now, on the calling thread. Used when the process is about
/// to die: the last planned state goes to disk before the panic continues.
pub fn write_now(root: &Path, id: &str, title: &str, doc: &Document) -> Result<(), String> {
    write_from(Planner::unsaved(), root, id, title, doc)
}

/// [`write_now`] relative to `planner`'s base (the saved package), so a restore keeps the text
/// sources of the save. Every change since the save is written again.
fn write_from(mut planner: Planner, root: &Path, id: &str, title: &str, doc: &Document) -> Result<(), String> {
    planner.mark_broken();
    // Tile names of this write never collide with the ones the worker writes.
    planner.seq = 1 << 32;
    let plan = planner.plan(doc, root, id, title);
    execute_locked(&plan)
}

/// Remove a document's journal (after a save, or when the user discards it).
pub fn remove(root: &Path, id: &str) -> Result<(), String> {
    match fs::remove_dir_all(root.join(id)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("{}: {e}", root.join(id).display())),
    }
}

fn encode_tile(tile: &Tile) -> Vec<u8> {
    let mut out = Vec::with_capacity(5 + tile.byte_len());
    out.extend_from_slice(TILE_MAGIC);
    match tile {
        Tile::U8(px) => {
            out.push(8);
            out.extend_from_slice(px);
        }
        Tile::U16(px) => {
            out.push(16);
            for v in px.iter() {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
    }
    out
}

fn decode_tile(bytes: &[u8]) -> Result<Tile, String> {
    if bytes.len() < 5 || &bytes[..4] != TILE_MAGIC {
        return Err("journal: a tile file is not a journal tile".to_owned());
    }
    let body = &bytes[5..];
    let tile = match bytes[4] {
        8 => Tile::from_u8(body.to_vec()),
        16 if body.len().is_multiple_of(2) => {
            Tile::from_u16(body.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes([c[0], c[1]])).collect())
        }
        _ => None,
    };
    tile.ok_or_else(|| "journal: a tile file has the wrong size".to_owned())
}

// ---------------------------------------------------------------- reading (start-up, restore)

/// Journals in `recovery` that are newer than their package. Older ones are stale (the save
/// that followed them already has their content) and are removed.
pub fn scan(recovery: &Path, library: &Library) -> Vec<Pending> {
    let Ok(dirs) = fs::read_dir(recovery) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for dir in dirs.flatten().filter(|d| d.path().is_dir()) {
        let Some(id) = dir.file_name().to_str().map(str::to_owned) else { continue };
        let Ok(text) = fs::read(dir.path().join(MANIFEST)) else { continue };
        let Some(m) = serde_json::from_slice::<Value>(&text).ok().filter(|m| m["format"].as_u64() == Some(FORMAT)) else {
            continue;
        };
        let saved_ms = m["saved_at_ms"].as_u64().unwrap_or(0);
        let saved_at = UNIX_EPOCH + Duration::from_millis(saved_ms);
        let package_ms = library
            .get(&id)
            .and_then(|e| e.modified.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64);
        if package_ms.is_some_and(|p| saved_ms <= p) {
            let _ = remove(recovery, &id);
            continue;
        }
        let title = m["title"].as_str().unwrap_or("Untitled").to_owned();
        out.push(Pending { id, title, saved_at });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// Rebuild the document of `id` from its package and its journal in `recovery`.
pub fn restore(recovery: &Path, library: &Library, id: &str) -> Result<Document, String> {
    restore_counted(recovery, library, id).map(|(doc, _)| doc)
}

/// Like [`restore`], and also the number of tiles that could not be read (those keep the
/// pixels of the saved package, or stay empty).
pub fn restore_counted(recovery: &Path, library: &Library, id: &str) -> Result<(Document, usize), String> {
    let mut skipped = 0usize;
    let dir = recovery.join(id);
    let text = fs::read(dir.join(MANIFEST)).map_err(|e| format!("{}: {e}", dir.display()))?;
    let m: Value = serde_json::from_slice(&text).map_err(|e| format!("journal: {e}"))?;
    if m["format"].as_u64() != Some(FORMAT) {
        return Err("journal: unknown format".to_owned());
    }
    let width = m["width"].as_u64().ok_or("journal: no width")? as usize;
    let height = m["height"].as_u64().ok_or("journal: no height")? as usize;
    let depth = m["depth"].as_u64().and_then(|b| Depth::from_bits(b as u32)).ok_or("journal: bad depth")?;
    let mut doc = Document::empty_depth(width, height, depth);
    doc.dpi = m["dpi"].as_f64().unwrap_or(doc.dpi);
    doc.colour_space = m["colour_space"].as_str().map(ColourSpace::from_profile_name).unwrap_or_default();
    if let Some(bg) = m["background"].as_array().filter(|a| a.len() == 3) {
        for (dst, v) in doc.background.iter_mut().zip(bg) {
            *dst = v.as_f64().unwrap_or(1.0);
        }
    }
    doc.background_visible = m["background_visible"].as_bool().unwrap_or(true);
    doc.root = tree_from_json(&m["tree"])?;
    // New items must not reuse an id from the journal.
    let top = max_id(&doc.root);
    while doc.alloc_id() < top {}

    // The base: the package's layers, matched to the journal's layer ids by position.
    let base = match m["base_keys"].as_array() {
        Some(keys) => {
            let keys: Vec<u64> = keys.iter().filter_map(Value::as_u64).collect();
            let package = library.load(id)?;
            if package.layer_count() != keys.len() {
                return Err("journal: the package does not match the journal".to_owned());
            }
            // A package of another depth or size cannot be the base (an older journal written
            // after Crop & Resize still names it).
            (package.depth == depth && (package.width, package.height) == (width, height)).then_some((keys, package))
        }
        None => None,
    };
    if let Some((_, package)) = &base {
        doc.animation = package.animation.clone();
    }

    // Tile records, grouped by layer.
    let mut records: HashMap<u64, Vec<(usize, usize, Option<String>)>> = HashMap::new();
    for r in m["records"].as_array().into_iter().flatten() {
        let (Some(layer), Some(tx), Some(ty)) = (r["layer"].as_u64(), r["tx"].as_u64(), r["ty"].as_u64()) else {
            continue;
        };
        let file = r["file"].as_str().map(str::to_owned);
        if let Some(name) = &file {
            if name.contains(['/', '\\']) || name.contains("..") {
                return Err("journal: a tile name is not a plain file name".to_owned());
            }
        }
        records.entry(layer).or_default().push((tx as usize, ty as usize, file));
    }

    for key in doc.layer_ids() {
        let mut layer = match base.as_ref().and_then(|(keys, package)| {
            let i = keys.iter().position(|k| *k == key)?;
            package.layer_at(i).cloned()
        }) {
            Some(l) => l,
            None => Layer::new_depth(width, height, depth),
        };
        for (tx, ty, file) in records.remove(&key).unwrap_or_default() {
            let tile = match file {
                None => None,
                Some(name) => {
                    match fs::read(dir.join(&name)).map_err(|e| e.to_string()).and_then(|b| decode_tile(&b)) {
                        Ok(t) => Some(Arc::new(t)),
                        Err(_) => {
                            skipped += 1;
                            continue;
                        }
                    }
                }
            };
            layer.set_tile(tx, ty, tile);
        }
        doc.put_layer(key, layer);
    }

    // Masks: a journal that lists them has every mask of the snapshot. An older one has none,
    // and its masked layers keep the mask they were saved with (below).
    let journal_masks = m["masks"].as_array();
    for e in journal_masks.into_iter().flatten() {
        let (Some(node), Some(file)) = (e["node"].as_u64(), e["file"].as_str()) else {
            continue;
        };
        if file.contains(['/', '\\']) || file.contains("..") {
            return Err("journal: a mask name is not a plain file name".to_owned());
        }
        match fs::read(dir.join(file)) {
            Ok(b) if b.len() == 4 + width * height && &b[..4] == MASK_MAGIC => {
                if let Some(n) = doc.node_mut(node) {
                    n.mask = Some(eskiz_core::document::Mask { data: b[4..].to_vec() });
                }
            }
            _ => skipped += 1,
        }
    }

    // Text layers keep the source, and masked layers the mask, they were saved with.
    if let Some((keys, package)) = &base {
        for (i, key) in keys.iter().enumerate() {
            if let (Some(pid), true) = (package.layer_id(i), doc.layer(*key).is_some()) {
                if let Some(text) = package.texts.get(&pid) {
                    doc.texts.insert(*key, text.clone());
                }
                if let Some(mask) = package.node(pid).and_then(|n| n.mask.clone()).filter(|_| journal_masks.is_none()) {
                    if let Some(node) = doc.node_mut(*key) {
                        node.mask = Some(mask);
                    }
                }
            }
        }
    }
    Ok((doc, skipped))
}

/// Delete a journal without restoring it.
pub fn discard(recovery: &Path, id: &str) -> Result<(), String> {
    remove(recovery, id)
}

/// The time a journal was written, for the start-up sheet.
pub fn when(t: SystemTime) -> String {
    use chrono::{DateTime, Local};
    let local: DateTime<Local> = t.into();
    local.format("%-d %b %Y at %H:%M").to_string()
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO).as_millis() as u64
}

// ---------------------------------------------------------------- panic flush

/// The last planned snapshot, for a panic to write before the process ends.
struct PanicSlot {
    root: PathBuf,
    id: String,
    title: String,
    doc: Box<Document>,
    /// The UI's planner after that plan: its base is the saved package.
    planner: Box<Planner>,
}

static PANIC_SLOT: Mutex<Option<PanicSlot>> = Mutex::new(None);

/// Remember the document planned last, so [`flush_for_panic`] can write it.
pub fn set_panic_snapshot(root: &Path, id: &str, title: &str, doc: &Document, planner: &Planner) {
    if let Ok(mut slot) = PANIC_SLOT.lock() {
        *slot = Some(PanicSlot {
            root: root.to_owned(),
            id: id.to_owned(),
            title: title.to_owned(),
            doc: Box::new(doc.clone()),
            planner: Box::new(planner.clone()),
        });
    }
}

/// Forget the remembered snapshot unless it is of document `id`. The UI calls it with `None`
/// once the document is saved (the package is then newer than the snapshot) or put away, so a
/// later panic cannot write an old state that looks newer than the package.
pub fn keep_panic_snapshot_for(id: Option<&str>) {
    if let Ok(mut slot) = PANIC_SLOT.try_lock() {
        if slot.as_ref().is_some_and(|s| Some(s.id.as_str()) != id) {
            *slot = None;
        }
    }
}

/// Write the remembered snapshot now, on this thread. Does nothing if the slot is locked.
pub fn flush_for_panic() {
    let Ok(slot) = PANIC_SLOT.try_lock() else {
        return;
    };
    // If the worker is in the middle of a write (or this is the worker), its last complete
    // manifest stays valid; do not write over it.
    let _writing = match WRITE_LOCK.try_lock() {
        Ok(g) => g,
        Err(std::sync::TryLockError::Poisoned(e)) => e.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => return,
    };
    if let Some(s) = slot.as_ref() {
        let _ = write_from((*s.planner).clone(), &s.root, &s.id, &s.title, &s.doc);
    }
}

/// Install the panic hook once: the journal is flushed before the previous hook runs. Only a
/// panic on the thread that installs it (the UI thread) flushes: worker threads catch their own
/// panics and carry on, and a flush then would rewrite the journal under the UI's planner.
pub fn install_panic_hook() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    // Android runs each activity on a new thread: the latest caller is the UI thread.
    if let Ok(mut ui) = UI_THREAD.lock() {
        *ui = Some(std::thread::current().id());
    }
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let on_ui = UI_THREAD.try_lock().ok().and_then(|ui| *ui) == Some(std::thread::current().id());
            if on_ui {
                let _ = std::panic::catch_unwind(flush_for_panic);
            }
            previous(info);
        }));
    });
}

/// The thread that last installed the panic hook: the UI thread.
static UI_THREAD: Mutex<Option<std::thread::ThreadId>> = Mutex::new(None);

// ---------------------------------------------------------------- selftest

/// `--selftest` check: a document is saved, edited in two steps (each journal writes only
/// its changed tiles), "crashes" (nothing but the disk is kept), and restores pixel for pixel.
/// Then a discard removes the journal.
pub fn selftest() -> Result<String, String> {
    let dir = std::env::temp_dir().join(format!("eskiz-selftest-journal-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let result = selftest_in(&dir);
    let _ = fs::remove_dir_all(&dir);
    result
}

fn selftest_in(dir: &Path) -> Result<String, String> {
    let lib = Library::at(dir);
    let root = lib.recovery_root();
    let mut doc = crate::storage::sample_document(600, 400);
    let id = crate::storage::new_id();
    lib.save(&id, "Crash", &doc)?;
    let mut planner = Planner::based_on(&doc);
    let (tiles_a, tiles_b) = {
        paint(&mut doc, 0, 10, 10, 20, 20);
        let a = planner.plan(&doc, root, &id, "Crash");
        execute(&a)?;
        paint(&mut doc, 1, 400, 300, 20, 20);
        let b = planner.plan(&doc, root, &id, "Crash");
        execute(&b)?;
        (a.tiles.len(), b.tiles.len())
    };
    if tiles_a != 1 || tiles_b != 1 {
        return Err(format!("journal: expected one changed tile per step, got {tiles_a} and {tiles_b}"));
    }
    drop(planner);
    let back = restore(root, &lib, &id)?;
    let same = back.layer_count() == doc.layer_count()
        && (0..doc.layer_count()).all(|i| back.layer_at(i).map(Layer::to_rgba) == doc.layer_at(i).map(Layer::to_rgba));
    if !same {
        return Err("journal: restored layers differ from the crashed document".to_owned());
    }
    discard(root, &id)?;
    if root.join(&id).exists() {
        return Err("journal: discard left the journal behind".to_owned());
    }
    Ok("  crash recovery: 2 journal steps wrote 1 tile each, restore after a crash is pixel-identical, discard removes the journal: ok".to_owned())
}

/// Paint a solid rectangle on layer `index` (used by the selftest and the tests).
pub(crate) fn paint(doc: &mut Document, index: usize, x: usize, y: usize, w: usize, h: usize) {
    let layer = doc.layer_at_mut(index).expect("layer");
    for yy in y..y + h {
        for xx in x..x + w {
            layer.set_pixel(xx, yy, [200, 40, 90, 255]);
        }
    }
}

#[cfg(test)]
#[path = "journal_tests.rs"]
mod tests;
