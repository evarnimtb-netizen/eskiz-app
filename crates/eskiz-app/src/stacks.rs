//! The library index: root order, stacks and the gallery sort.
//!
//! The index is one JSON file, `library.json`, in the library folder (see `storage`). It lists
//! the root items of the gallery in order, each a document or a stack, and the documents inside
//! every stack. A document is in the index exactly once. [`LibraryIndex::reconcile`] brings the
//! index in line with the folder: new documents go to the front of the root list, and entries for
//! documents that have gone are dropped.
//!
//! Eskiz's rules, where the design notes leave a choice:
//! * moving a document into a stack puts it first in the stack;
//! * a stack needs two documents; one left alone is dissolved and returns to the root list;
//! * a document taken out of a stack goes to the root list at the position it was dropped.
//!
//! Pure data with no egui dependency. The file itself is read and written by `storage`.

use std::collections::HashSet;
use std::time::SystemTime;

use serde_json::{json, Value};

/// A root-level gallery item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    Doc(String),
    Stack(String),
}

impl Item {
    pub fn id(&self) -> &str {
        match self {
            Item::Doc(id) | Item::Stack(id) => id,
        }
    }
}

/// A named folder of documents. The first document is the first one shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stack {
    pub id: String,
    pub name: String,
    pub docs: Vec<String>,
}

/// How the gallery orders its cards. Manual is the order the user dragged them into.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortMode {
    #[default]
    Manual,
    Name,
    Modified,
    Size,
}

impl SortMode {
    pub const ALL: [SortMode; 4] = [SortMode::Manual, SortMode::Name, SortMode::Modified, SortMode::Size];

    pub fn label(self) -> &'static str {
        match self {
            SortMode::Manual => "Manual",
            SortMode::Name => "Name",
            SortMode::Modified => "Date modified",
            SortMode::Size => "Size",
        }
    }

    fn key(self) -> &'static str {
        match self {
            SortMode::Manual => "manual",
            SortMode::Name => "name",
            SortMode::Modified => "modified",
            SortMode::Size => "size",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        SortMode::ALL.into_iter().find(|m| m.key() == key)
    }
}

/// What a card sorts by.
#[derive(Clone, Debug, PartialEq)]
pub struct SortKey {
    pub name: String,
    pub modified: SystemTime,
    /// Pixel area.
    pub area: u64,
}

/// Positions of `keys` in display order. Manual keeps the order given. Name sorts A to Z
/// (ignoring case), Date modified puts the newest first, and Size the largest first. Ties keep
/// their order.
pub fn display_order(keys: &[SortKey], mode: SortMode) -> Vec<usize> {
    let mut order: Vec<usize> = (0..keys.len()).collect();
    match mode {
        SortMode::Manual => {}
        SortMode::Name => order.sort_by_key(|&i| keys[i].name.to_lowercase()),
        SortMode::Modified => order.sort_by(|&a, &b| keys[b].modified.cmp(&keys[a].modified)),
        SortMode::Size => order.sort_by(|&a, &b| keys[b].area.cmp(&keys[a].area)),
    }
    order
}

/// The whole index.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LibraryIndex {
    pub order: Vec<Item>,
    pub stacks: Vec<Stack>,
    pub sort: SortMode,
}

impl LibraryIndex {
    pub fn stack(&self, id: &str) -> Option<&Stack> {
        self.stacks.iter().find(|s| s.id == id)
    }

    fn stack_mut(&mut self, id: &str) -> Option<&mut Stack> {
        self.stacks.iter_mut().find(|s| s.id == id)
    }

    pub fn is_stack(&self, id: &str) -> bool {
        self.stack(id).is_some()
    }

    /// The stack that holds `doc`, if any.
    pub fn stack_of(&self, doc: &str) -> Option<&str> {
        self.stacks.iter().find(|s| s.docs.iter().any(|d| d == doc)).map(|s| s.id.as_str())
    }

    /// "Stack", "Stack 2", ... the first name no stack uses.
    pub fn next_stack_name(&self) -> String {
        let taken: HashSet<&str> = self.stacks.iter().map(|s| s.name.as_str()).collect();
        if !taken.contains("Stack") {
            return "Stack".to_owned();
        }
        (2..).map(|n| format!("Stack {n}")).find(|t| !taken.contains(t.as_str())).unwrap_or_default()
    }

    /// Bring the index in line with the documents on disk. `docs` holds every document ID,
    /// newest first. Returns true when the index changed.
    pub fn reconcile(&mut self, docs: &[String]) -> bool {
        let before = self.clone();
        let present: HashSet<&str> = docs.iter().map(String::as_str).collect();
        let mut placed: HashSet<String> = HashSet::new();
        for s in &mut self.stacks {
            s.docs.retain(|d| present.contains(d.as_str()) && placed.insert(d.clone()));
        }
        self.order.retain(|item| match item {
            Item::Doc(id) => present.contains(id.as_str()) && placed.insert(id.clone()),
            Item::Stack(_) => true,
        });
        let fresh: Vec<Item> = docs.iter().filter(|d| !placed.contains(d.as_str())).map(|d| Item::Doc(d.clone())).collect();
        self.order.splice(0..0, fresh);
        self.normalize();
        *self != before
    }

    /// Keep the structure valid: stacks with fewer than two documents are dissolved, and every
    /// stack has exactly one entry in the root list.
    fn normalize(&mut self) {
        let mut dissolved = Vec::new();
        self.stacks.retain(|s| {
            let keep = s.docs.len() >= 2;
            if !keep {
                dissolved.push(s.clone());
            }
            keep
        });
        for s in dissolved {
            let replacement: Vec<Item> = s.docs.iter().map(|d| Item::Doc(d.clone())).collect();
            match self.order.iter().position(|it| *it == Item::Stack(s.id.clone())) {
                Some(p) => {
                    self.order.splice(p..=p, replacement);
                }
                None => {
                    self.order.splice(0..0, replacement);
                }
            }
        }
        let names: HashSet<String> = self.stacks.iter().map(|s| s.id.clone()).collect();
        self.order.retain(|it| match it {
            Item::Stack(id) => names.contains(id),
            Item::Doc(_) => true,
        });
        let listed: HashSet<String> = self
            .order
            .iter()
            .filter_map(|it| match it {
                Item::Stack(id) => Some(id.clone()),
                Item::Doc(_) => None,
            })
            .collect();
        let missing: Vec<Item> = self.stacks.iter().filter(|s| !listed.contains(&s.id)).map(|s| Item::Stack(s.id.clone())).collect();
        self.order.splice(0..0, missing);
    }

    /// Remove `doc` from wherever it is at the root level or in a stack, without dropping it.
    fn take_doc(&mut self, doc: &str) {
        for s in &mut self.stacks {
            s.docs.retain(|d| d != doc);
        }
        self.order.retain(|it| *it != Item::Doc(doc.to_owned()));
    }

    /// Put root document `doc` into `stack`, first.
    pub fn add_to_stack(&mut self, doc: &str, stack: &str) -> bool {
        if !self.is_stack(stack) || !self.order.contains(&Item::Doc(doc.to_owned())) {
            return false;
        }
        self.take_doc(doc);
        if let Some(s) = self.stack_mut(stack) {
            s.docs.insert(0, doc.to_owned());
        }
        self.normalize();
        true
    }

    /// Make a stack named `name` from root documents `docs`, in that order, where `anchor` (one
    /// of them) was in the root list.
    pub fn stack_docs(&mut self, id: String, name: String, docs: &[String], anchor: &str) -> bool {
        let root_docs: Vec<&String> = docs.iter().filter(|d| self.order.contains(&Item::Doc((*d).clone()))).collect();
        if root_docs.len() < 2 || !root_docs.iter().any(|d| d.as_str() == anchor) {
            return false;
        }
        let Some(at) = self.order.iter().position(|it| *it == Item::Doc(anchor.to_owned())) else {
            return false;
        };
        // Documents above the anchor leave the root list, so the stack lands that much earlier.
        let removed_before = self.order[..at].iter().filter(|it| matches!(it, Item::Doc(d) if root_docs.contains(&d))).count();
        let docs: Vec<String> = root_docs.into_iter().cloned().collect();
        for d in &docs {
            self.take_doc(d);
        }
        self.stacks.push(Stack { id: id.clone(), name, docs });
        let at = (at - removed_before).min(self.order.len());
        self.order.insert(at, Item::Stack(id));
        self.normalize();
        true
    }

    /// Take `doc` out of its stack and put it in the root list before `before` (at the end when
    /// `None`).
    pub fn unstack(&mut self, doc: &str, before: Option<&Item>) -> bool {
        if self.stack_of(doc).is_none() {
            return false;
        }
        for s in &mut self.stacks {
            s.docs.retain(|d| d != doc);
        }
        let at = before.and_then(|b| self.order.iter().position(|it| it == b)).unwrap_or(self.order.len());
        self.order.insert(at, Item::Doc(doc.to_owned()));
        self.normalize();
        true
    }

    /// Move a root item before `before` (to the end when `None`).
    pub fn move_before(&mut self, item: &Item, before: Option<&Item>) -> bool {
        if Some(item) == before || !self.order.contains(item) {
            return false;
        }
        self.order.retain(|it| it != item);
        let at = before.and_then(|b| self.order.iter().position(|it| it == b)).unwrap_or(self.order.len());
        self.order.insert(at, item.clone());
        true
    }

    /// Move document `doc` inside stack `stack` to before `before` (to the end when `None`).
    pub fn move_in_stack(&mut self, stack: &str, doc: &str, before: Option<&str>) -> bool {
        let Some(s) = self.stack_mut(stack) else {
            return false;
        };
        if !s.docs.iter().any(|d| d == doc) || before == Some(doc) {
            return false;
        }
        s.docs.retain(|d| d != doc);
        let at = before.and_then(|b| s.docs.iter().position(|d| d == b)).unwrap_or(s.docs.len());
        s.docs.insert(at, doc.to_owned());
        true
    }

    pub fn rename_stack(&mut self, id: &str, name: &str) -> bool {
        match self.stack_mut(id) {
            Some(s) if s.name != name => {
                s.name = name.to_owned();
                true
            }
            _ => false,
        }
    }

    /// The root item after `item`, if there is one.
    pub fn next_root(&self, item: &Item) -> Option<Item> {
        let at = self.order.iter().position(|it| it == item)?;
        self.order.get(at + 1).cloned()
    }

    /// The document after `doc` in its stack, if there is one.
    pub fn next_in_stack(&self, doc: &str) -> Option<String> {
        let s = self.stacks.iter().find(|s| s.docs.iter().any(|d| d == doc))?;
        let at = s.docs.iter().position(|d| d == doc)?;
        s.docs.get(at + 1).cloned()
    }

    /// The index as JSON, the format of `library.json`.
    pub fn encode(&self) -> String {
        let order: Vec<Value> = self
            .order
            .iter()
            .map(|it| match it {
                Item::Doc(id) => json!({ "doc": id }),
                Item::Stack(id) => json!({ "stack": id }),
            })
            .collect();
        let stacks: Vec<Value> = self.stacks.iter().map(|s| json!({ "id": s.id, "name": s.name, "docs": s.docs })).collect();
        json!({ "version": 1, "sort": self.sort.key(), "order": order, "stacks": stacks }).to_string()
    }

    /// Parse [`LibraryIndex::encode`] output. `None` when the text is not an index.
    pub fn decode(text: &str) -> Option<Self> {
        let v: Value = serde_json::from_str(text).ok()?;
        let order = v
            .get("order")?
            .as_array()?
            .iter()
            .filter_map(|e| match (e.get("doc").and_then(Value::as_str), e.get("stack").and_then(Value::as_str)) {
                (Some(d), _) => Some(Item::Doc(d.to_owned())),
                (None, Some(s)) => Some(Item::Stack(s.to_owned())),
                _ => None,
            })
            .collect();
        let stacks = v
            .get("stacks")?
            .as_array()?
            .iter()
            .filter_map(|s| {
                Some(Stack {
                    id: s.get("id")?.as_str()?.to_owned(),
                    name: s.get("name")?.as_str()?.to_owned(),
                    docs: s.get("docs")?.as_array()?.iter().filter_map(|d| d.as_str().map(str::to_owned)).collect(),
                })
            })
            .collect();
        let sort = v.get("sort").and_then(Value::as_str).and_then(SortMode::from_key).unwrap_or_default();
        Some(Self { order, stacks, sort })
    }
}

/// Stacks and the index file on disk, end to end: create, reopen, unstack, delete. Returns one
/// line for `--selftest`.
pub fn stacks_selftest() -> Result<String, String> {
    use crate::storage::{new_id, Library};
    use crate::document::Document;
    let dir = std::env::temp_dir().join(format!("eskiz-selftest-stacks-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let result = (|| {
        let lib = Library::at(&dir);
        let mut ids = Vec::new();
        for i in 0..3 {
            let id = new_id();
            lib.save(&id, &format!("Card {i}"), &Document::new(64, 48)).map_err(|e| format!("stacks: {e}"))?;
            ids.push(id);
        }
        let mut index = lib.read_index();
        let listed: Vec<String> = lib.list().into_iter().map(|e| e.id).collect();
        index.reconcile(&listed);
        let sid = new_id();
        let pair = vec![ids[0].clone(), ids[1].clone()];
        if !index.stack_docs(sid.clone(), "Pair".into(), &pair, &ids[0]) {
            return Err("stacks: could not create the stack".to_owned());
        }
        lib.write_index(&index)?;
        let back = Library::at(&dir).read_index();
        if back != index || back.stack(&sid).map(|s| s.docs.len()) != Some(2) {
            return Err("stacks: the stack did not survive a reopen".to_owned());
        }
        let mut again = back.clone();
        again.unstack(&ids[1], None);
        if again.is_stack(&sid) || again.order.len() != 3 {
            return Err("stacks: unstacking left a single-document stack".to_owned());
        }
        Ok(format!("gallery stacks: create, reopen, unstack ok ({} items at root)", again.order.len()))
    })();
    let _ = std::fs::remove_dir_all(&dir);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;

    fn doc(s: &str) -> Item {
        Item::Doc(s.to_owned())
    }

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    fn root(ix: &LibraryIndex) -> Vec<String> {
        ix.order.iter().map(|it| it.id().to_owned()).collect()
    }

    #[test]
    fn reconcile_adds_new_documents_at_the_front_and_drops_missing_ones() {
        let mut ix = LibraryIndex { order: vec![doc("a"), doc("gone"), doc("b")], ..Default::default() };
        assert!(ix.reconcile(&ids(&["new", "a", "b"])));
        assert_eq!(root(&ix), ["new", "a", "b"]);
        assert!(!ix.reconcile(&ids(&["new", "a", "b"])), "a second pass changes nothing");
    }

    #[test]
    fn stack_create_places_the_stack_at_the_anchor_and_keeps_the_order() {
        let mut ix = LibraryIndex { order: vec![doc("a"), doc("b"), doc("c"), doc("d")], ..Default::default() };
        assert!(ix.stack_docs("s1".into(), "Pair".into(), &ids(&["c", "a"]), "c"));
        assert_eq!(root(&ix), ["b", "s1", "d"], "the stack takes the anchor's place (after the document above it left)");
        assert_eq!(ix.stack("s1").unwrap().docs, ids(&["c", "a"]));
    }

    #[test]
    fn stack_needs_two_documents() {
        let mut ix = LibraryIndex { order: vec![doc("a"), doc("b")], ..Default::default() };
        assert!(!ix.stack_docs("s1".into(), "One".into(), &ids(&["a"]), "a"));
        assert_eq!(root(&ix), ["a", "b"]);
    }

    #[test]
    fn dropping_onto_a_stack_inserts_first_and_dissolves_a_lonely_source_stack() {
        let mut ix = LibraryIndex { order: vec![doc("a"), doc("b"), doc("c")], ..Default::default() };
        ix.stack_docs("s1".into(), "S".into(), &ids(&["a", "b"]), "a");
        assert!(ix.add_to_stack("c", "s1"));
        assert_eq!(ix.stack("s1").unwrap().docs, ids(&["c", "a", "b"]));
        assert_eq!(root(&ix), ["s1"]);
    }

    #[test]
    fn unstack_returns_the_document_and_dissolves_a_single_document_stack() {
        let mut ix = LibraryIndex { order: vec![doc("a"), doc("b"), doc("c")], ..Default::default() };
        ix.stack_docs("s1".into(), "S".into(), &ids(&["a", "b"]), "a");
        assert!(ix.unstack("b", None));
        assert!(!ix.is_stack("s1"), "one document left: the stack is dissolved");
        assert_eq!(root(&ix), ["a", "c", "b"], "no position given: at the end");
        assert!(!ix.unstack("c", None), "a root document is not in a stack");
    }

    #[test]
    fn stack_survives_encode_and_decode() {
        let mut ix = LibraryIndex { order: vec![doc("a"), doc("b"), doc("c")], sort: SortMode::Size, ..Default::default() };
        ix.stack_docs("s1".into(), "Trip".into(), &ids(&["a", "c"]), "a");
        ix.rename_stack("s1", "Trip 2");
        assert_eq!(LibraryIndex::decode(&ix.encode()), Some(ix));
        assert_eq!(LibraryIndex::decode("not json"), None);
    }

    #[test]
    fn move_root_and_move_in_stack() {
        let mut ix = LibraryIndex { order: vec![doc("a"), doc("b"), doc("c")], ..Default::default() };
        assert!(ix.move_before(&doc("c"), Some(&doc("a"))));
        assert_eq!(root(&ix), ["c", "a", "b"]);
        assert!(ix.move_before(&doc("a"), None));
        assert_eq!(root(&ix), ["c", "b", "a"]);
        ix.stack_docs("s1".into(), "S".into(), &ids(&["c", "a", "b"]), "c");
        assert!(ix.move_in_stack("s1", "b", Some("c")));
        assert_eq!(ix.stack("s1").unwrap().docs, ids(&["b", "c", "a"]));
    }

    #[test]
    fn rename_stack_changes_only_the_name() {
        let mut ix = LibraryIndex { order: vec![doc("a"), doc("b")], ..Default::default() };
        ix.stack_docs("s1".into(), "S".into(), &ids(&["a", "b"]), "a");
        assert!(ix.rename_stack("s1", "Sketches"));
        assert_eq!(ix.stack("s1").unwrap().name, "Sketches");
        assert_eq!(ix.next_stack_name(), "Stack");
    }

    #[test]
    fn sort_orders() {
        let t0 = UNIX_EPOCH;
        let keys = vec![
            SortKey { name: "beta".into(), modified: t0 + std::time::Duration::from_secs(5), area: 100 },
            SortKey { name: "Alpha".into(), modified: t0 + std::time::Duration::from_secs(9), area: 400 },
            SortKey { name: "gamma".into(), modified: t0 + std::time::Duration::from_secs(1), area: 900 },
        ];
        assert_eq!(display_order(&keys, SortMode::Manual), vec![0, 1, 2]);
        assert_eq!(display_order(&keys, SortMode::Name), vec![1, 0, 2]);
        assert_eq!(display_order(&keys, SortMode::Modified), vec![1, 0, 2]);
        assert_eq!(display_order(&keys, SortMode::Size), vec![2, 1, 0]);
    }

    #[test]
    fn stacks_round_trip_on_disk() {
        assert!(stacks_selftest().is_ok(), "{:?}", stacks_selftest());
    }
}
