//! glTF/GLB and OBJ import. Each material becomes a [`TextureSet`].

use std::fmt;
use std::io::Cursor;
use std::path::{Component, Path, PathBuf};

use glam::{Mat3, Mat4, Vec2, Vec3};

use crate::check::CheckReport;
use crate::model::{linear_to_srgb, srgb_to_linear, MapKind, Model, SubMesh, TextureSet};

/// Import settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImportOptions {
    /// Map size for materials with no texture.
    pub default_texture_size: usize,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            default_texture_size: 1024,
        }
    }
}

/// A successful import: the model plus its non-fatal check report.
#[derive(Clone, Debug)]
pub struct ImportedModel {
    pub model: Model,
    /// Contains warnings only; fatal findings make the import fail instead.
    pub report: CheckReport,
    /// True if any mesh lacked normals and flat normals were generated.
    pub normals_generated: bool,
    /// Non-fatal import remarks (for example a texture file that could not be read).
    pub notes: Vec<String>,
}

/// Import failure.
#[derive(Clone, Debug, PartialEq)]
pub enum ImportError {
    Io(String),
    Parse(String),
    /// The file uses a feature the importer does not support; the text is shown to the user.
    Unsupported(String),
    /// A fatal model check failed; the report names the problem.
    Rejected(CheckReport),
}

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImportError::Io(s) => write!(f, "I/O error: {s}"),
            ImportError::Parse(s) => write!(f, "parse error: {s}"),
            ImportError::Unsupported(s) => write!(f, "{s}"),
            ImportError::Rejected(r) => write!(f, "model rejected: {}", r.messages().join(" ")),
        }
    }
}

impl std::error::Error for ImportError {}

// ---------------------------------------------------------------------------
// Shared texture-set assembly
// ---------------------------------------------------------------------------

/// Decoded straight-alpha RGBA8 image.
#[derive(Clone, Debug)]
pub(crate) struct Img {
    pub w: usize,
    pub h: usize,
    pub rgba: Vec<u8>,
}

/// Largest texture side kept after import; bigger pictures are sampled down.
pub(crate) const MAX_TEXTURE_SIDE: usize = 8192;
/// Largest picture side accepted by the decoder.
const MAX_DECODE_SIDE: u32 = 16384;

/// Decode a texture with explicit size and memory limits.
pub(crate) fn decode_texture(
    reader: image::ImageReader<impl std::io::BufRead + std::io::Seek>,
) -> Result<image::DynamicImage, image::ImageError> {
    let mut reader = reader;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DECODE_SIDE);
    limits.max_image_height = Some(MAX_DECODE_SIDE);
    limits.max_alloc = Some(512 * 1024 * 1024);
    reader.limits(limits);
    reader.decode()
}

impl Img {
    fn at(&self, x: usize, y: usize, size: (usize, usize)) -> [u8; 4] {
        // Nearest-neighbour resample to `size`.
        let sx = (x * self.w / size.0).min(self.w - 1);
        let sy = (y * self.h / size.1).min(self.h - 1);
        let i = (sy * self.w + sx) * 4;
        [
            self.rgba[i],
            self.rgba[i + 1],
            self.rgba[i + 2],
            self.rgba[i + 3],
        ]
    }
}

/// Everything known about one material before it becomes a [`TextureSet`].
#[derive(Default)]
pub(crate) struct MaterialSources {
    pub name: String,
    /// Linear RGBA factor.
    pub colour_factor: [f32; 4],
    pub roughness_factor: f32,
    pub metallic_factor: f32,
    pub colour: Option<Img>,
    /// Image and the channel (0..3) holding the value.
    pub roughness: Option<(Img, usize)>,
    pub metallic: Option<(Img, usize)>,
    pub normal: Option<Img>,
    pub ao: Option<(Img, usize)>,
    /// The material is opaque (glTF `alphaMode` OPAQUE): the colour texture's alpha is not
    /// coverage (exporters often pack other data there), so it is read as fully opaque.
    pub opaque: bool,
}

pub(crate) fn assemble(src: MaterialSources, default_size: usize) -> TextureSet {
    let default_size = default_size.clamp(1, 4096);
    let size = [
        src.colour.as_ref(),
        src.roughness.as_ref().map(|r| &r.0),
        src.metallic.as_ref().map(|r| &r.0),
        src.normal.as_ref(),
        src.ao.as_ref().map(|r| &r.0),
    ]
    .into_iter()
    .flatten()
    .map(|i| (i.w, i.h))
    .next()
    .map_or((default_size, default_size), |(w, h)| {
        (w.clamp(1, MAX_TEXTURE_SIDE), h.clamp(1, MAX_TEXTURE_SIDE))
    });

    let colour_const = {
        let f = src.colour_factor;
        [
            linear_to_srgb(f[0]),
            linear_to_srgb(f[1]),
            linear_to_srgb(f[2]),
            (f[3].clamp(0.0, 1.0) * 255.0).round() as u8,
        ]
    };
    let mut set = TextureSet::from_constants(
        &src.name,
        size.0,
        size.1,
        colour_const,
        src.roughness_factor,
        src.metallic_factor,
    );

    if let Some(img) = &src.colour {
        let f = src.colour_factor;
        let tint = f != [1.0, 1.0, 1.0, 1.0];
        let mut buf = vec![0u8; size.0 * size.1 * 4];
        for y in 0..size.1 {
            for x in 0..size.0 {
                let mut p = img.at(x, y, size);
                if tint {
                    for c in 0..3 {
                        p[c] = linear_to_srgb(srgb_to_linear(p[c]) * f[c]);
                    }
                    p[3] = (f32::from(p[3]) * f[3].clamp(0.0, 1.0)).round() as u8;
                }
                if src.opaque {
                    p[3] = 255;
                }
                let a = u32::from(p[3]);
                let i = (y * size.0 + x) * 4;
                for (c, &v) in p.iter().enumerate().take(3) {
                    buf[i + c] = ((u32::from(v) * a + 127) / 255) as u8;
                }
                buf[i + 3] = p[3];
            }
        }
        set.map_mut(MapKind::Colour)
            .write_rect(0, 0, size.0, size.1, &buf);
    }
    let mut channel_map = |kind: MapKind, src_img: &Option<(Img, usize)>, factor: f32| {
        if let Some((img, ch)) = src_img {
            let mut buf = vec![0u8; size.0 * size.1 * 4];
            for y in 0..size.1 {
                for x in 0..size.0 {
                    let v = (f32::from(img.at(x, y, size)[*ch]) * factor)
                        .round()
                        .clamp(0.0, 255.0) as u8;
                    let i = (y * size.0 + x) * 4;
                    buf[i..i + 4].copy_from_slice(&[v, v, v, 255]);
                }
            }
            set.map_mut(kind).write_rect(0, 0, size.0, size.1, &buf);
        }
    };
    channel_map(MapKind::Roughness, &src.roughness, src.roughness_factor);
    channel_map(MapKind::Metallic, &src.metallic, src.metallic_factor);
    channel_map(MapKind::AmbientOcclusion, &src.ao, 1.0);
    if let Some(img) = &src.normal {
        let mut buf = vec![0u8; size.0 * size.1 * 4];
        for y in 0..size.1 {
            for x in 0..size.0 {
                let p = img.at(x, y, size);
                let i = (y * size.0 + x) * 4;
                buf[i..i + 4].copy_from_slice(&[p[0], p[1], p[2], 255]);
            }
        }
        set.map_mut(MapKind::Normal)
            .write_rect(0, 0, size.0, size.1, &buf);
    }
    set
}

/// Run checks and wrap up an import.
pub(crate) fn finish(
    model: Model,
    normals_generated: bool,
    notes: Vec<String>,
) -> Result<ImportedModel, ImportError> {
    let report = model.check();
    if report.is_fatal() {
        return Err(ImportError::Rejected(report));
    }
    Ok(ImportedModel {
        model,
        report,
        normals_generated,
        notes,
    })
}

fn default_set(opts: &ImportOptions) -> TextureSet {
    assemble(
        MaterialSources {
            name: "default".to_string(),
            colour_factor: [1.0, 1.0, 1.0, 1.0],
            roughness_factor: 1.0,
            metallic_factor: 0.0,
            ..MaterialSources::default()
        },
        opts.default_texture_size,
    )
}

// ---------------------------------------------------------------------------
// glTF
// ---------------------------------------------------------------------------

/// Load a `.gltf` or `.glb` file (external buffers and images resolve next to it).
pub fn load_gltf(path: &Path, opts: &ImportOptions) -> Result<ImportedModel, ImportError> {
    // External buffers and images must stay in the model's folder: a file could otherwise name
    // any file on the disk (or a device that never ends).
    let gltf = gltf::Gltf::open(path).map_err(|e| ImportError::Parse(e.to_string()))?;
    let buffer_uris = gltf.buffers().filter_map(|b| match b.source() {
        gltf::buffer::Source::Uri(u) => Some(u),
        gltf::buffer::Source::Bin => None,
    });
    let image_uris = gltf.images().filter_map(|i| match i.source() {
        gltf::image::Source::Uri { uri, .. } => Some(uri),
        gltf::image::Source::View { .. } => None,
    });
    for uri in buffer_uris.chain(image_uris) {
        if !uri_stays_inside(uri) {
            return Err(ImportError::Unsupported(format!("the model refers to a file outside its folder ({uri})")));
        }
    }
    drop(gltf);
    let (doc, buffers, images) =
        gltf::import(path).map_err(|e| ImportError::Parse(e.to_string()))?;
    build_gltf(&doc, &buffers, &images, opts)
}

/// Load glTF/GLB bytes. External URIs are not resolved; data URIs and GLB chunks are.
pub fn load_gltf_slice(bytes: &[u8], opts: &ImportOptions) -> Result<ImportedModel, ImportError> {
    let (doc, buffers, images) =
        gltf::import_slice(bytes).map_err(|e| ImportError::Parse(e.to_string()))?;
    build_gltf(&doc, &buffers, &images, opts)
}

fn gltf_image(images: &[gltf::image::Data], index: usize) -> Option<Img> {
    use gltf::image::Format as F;
    let d = images.get(index)?;
    let n = d.width as usize * d.height as usize;
    let mut rgba = Vec::with_capacity(n * 4);
    let p = &d.pixels;
    match d.format {
        F::R8 => p.iter().for_each(|&v| rgba.extend([v, v, v, 255])),
        F::R8G8 => p
            .chunks_exact(2)
            .for_each(|c| rgba.extend([c[0], c[1], 0, 255])),
        F::R8G8B8 => p
            .chunks_exact(3)
            .for_each(|c| rgba.extend([c[0], c[1], c[2], 255])),
        F::R8G8B8A8 => rgba.extend_from_slice(p),
        F::R16 => p
            .chunks_exact(2)
            .for_each(|c| rgba.extend([c[1], c[1], c[1], 255])),
        F::R16G16 => p
            .chunks_exact(4)
            .for_each(|c| rgba.extend([c[1], c[3], 0, 255])),
        F::R16G16B16 => p
            .chunks_exact(6)
            .for_each(|c| rgba.extend([c[1], c[3], c[5], 255])),
        F::R16G16B16A16 => p
            .chunks_exact(8)
            .for_each(|c| rgba.extend([c[1], c[3], c[5], c[7]])),
        _ => return None,
    }
    (rgba.len() == n * 4 && n > 0).then_some(Img {
        w: d.width as usize,
        h: d.height as usize,
        rgba,
    })
}

fn build_gltf(
    doc: &gltf::Document,
    buffers: &[gltf::buffer::Data],
    images: &[gltf::image::Data],
    opts: &ImportOptions,
) -> Result<ImportedModel, ImportError> {
    // The glTF reader checks a buffer view's stride only against 4..=252: a stride shorter than
    // the element makes its accessor iterator panic on the first item.
    for acc in doc.accessors() {
        if let Some(stride) = acc.view().and_then(|v| v.stride()) {
            if stride < acc.size() {
                return Err(ImportError::Parse(format!(
                    "accessor {} has elements of {} bytes but a stride of {stride}",
                    acc.index(),
                    acc.size()
                )));
            }
        }
    }
    let mut notes = Vec::new();
    let (mut lines, mut no_positions) = (0usize, 0usize);
    let img = |tex: gltf::Texture<'_>| gltf_image(images, tex.source().index());
    let mut sets: Vec<TextureSet> = doc
        .materials()
        .map(|m| {
            let pbr = m.pbr_metallic_roughness();
            let mr = pbr
                .metallic_roughness_texture()
                .and_then(|i| img(i.texture()));
            let name = m.name().map_or_else(
                || format!("material{}", m.index().unwrap_or(0)),
                str::to_string,
            );
            let src = MaterialSources {
                name,
                colour_factor: pbr.base_color_factor(),
                roughness_factor: pbr.roughness_factor(),
                metallic_factor: pbr.metallic_factor(),
                colour: pbr.base_color_texture().and_then(|i| img(i.texture())),
                roughness: mr.clone().map(|i| (i, 1)),
                metallic: mr.map(|i| (i, 2)),
                normal: m.normal_texture().and_then(|i| img(i.texture())),
                ao: m
                    .occlusion_texture()
                    .and_then(|i| img(i.texture()))
                    .map(|i| (i, 0)),
                opaque: m.alpha_mode() == gltf::material::AlphaMode::Opaque,
            };
            assemble(src, opts.default_texture_size)
        })
        .collect();
    let material_count = sets.len();
    let mut default_index: Option<usize> = None;

    let mut submeshes = Vec::new();
    let mut points = 0usize;
    let mut generated = false;

    let mut visit =
        |mesh: gltf::Mesh<'_>, xf: Mat4, sets: &mut Vec<TextureSet>| -> Result<(), ImportError> {
            let nm = Mat3::from_mat4(xf).inverse().transpose();
            let flip = xf.determinant() < 0.0;
            for (pi, prim) in mesh.primitives().enumerate() {
                use gltf::mesh::Mode;
                if prim.mode() == Mode::Points {
                    points += 1;
                    continue;
                }
                if !matches!(
                    prim.mode(),
                    Mode::Triangles | Mode::TriangleStrip | Mode::TriangleFan
                ) {
                    lines += 1;
                    continue;
                }
                let reader = prim.reader(|b| buffers.get(b.index()).map(|d| &d.0[..]));
                let Some(pos) = reader.read_positions() else {
                    no_positions += 1;
                    continue;
                };
                let positions: Vec<Vec3> =
                    pos.map(|p| xf.transform_point3(Vec3::from(p))).collect();
                let normals: Vec<Vec3> = reader
                    .read_normals()
                    .map(|n| {
                        n.map(|v| (nm * Vec3::from(v)).normalize_or_zero())
                            .collect()
                    })
                    .unwrap_or_default();
                let uvs: Vec<Vec2> = reader
                    .read_tex_coords(0)
                    .map(|t| t.into_f32().map(Vec2::from).collect())
                    .unwrap_or_default();
                let raw: Vec<u32> = match reader.read_indices() {
                    Some(i) => i.into_u32().collect(),
                    None => (0..positions.len() as u32).collect(),
                };
                let mut indices = Vec::with_capacity(raw.len());
                match prim.mode() {
                    Mode::Triangles => indices.extend(raw.chunks_exact(3).flatten().copied()),
                    Mode::TriangleStrip => {
                        for i in 2..raw.len() {
                            let (a, b, c) = (raw[i - 2], raw[i - 1], raw[i]);
                            if i % 2 == 0 {
                                indices.extend([a, b, c]);
                            } else {
                                indices.extend([b, a, c]);
                            }
                        }
                    }
                    _ => {
                        for i in 2..raw.len() {
                            indices.extend([raw[0], raw[i - 1], raw[i]]);
                        }
                    }
                }
                if flip {
                    indices.chunks_exact_mut(3).for_each(|t| t.swap(1, 2));
                }
                let n = positions.len() as u32;
                if indices.iter().any(|&i| i >= n)
                    || (!uvs.is_empty() && uvs.len() != positions.len())
                    || (!normals.is_empty() && normals.len() != positions.len())
                {
                    return Err(ImportError::Parse(
                        "mesh attribute or index out of range".into(),
                    ));
                }
                let material = match prim.material().index() {
                    Some(i) => i,
                    None => *default_index.get_or_insert_with(|| {
                        sets.push(default_set(opts));
                        sets.len() - 1
                    }),
                };
                let name = mesh
                    .name()
                    .map_or_else(|| format!("mesh{}", mesh.index()), str::to_string);
                let mut sm = SubMesh {
                    name: if mesh.primitives().len() > 1 {
                        format!("{name}.{pi}")
                    } else {
                        name
                    },
                    positions,
                    normals,
                    uvs,
                    indices,
                    material,
                };
                if sm.normals.is_empty() && sm.triangle_count() > 0 {
                    sm.generate_flat_normals();
                    generated = true;
                }
                submeshes.push(sm);
            }
            Ok(())
        };

    let scene = doc.default_scene().or_else(|| doc.scenes().next());
    if let Some(scene) = scene {
        let mut stack: Vec<(gltf::Node<'_>, Mat4)> =
            scene.nodes().map(|n| (n, Mat4::IDENTITY)).collect();
        stack.reverse();
        let mut seen = vec![false; doc.nodes().len()];
        while let Some((node, parent)) = stack.pop() {
            // glTF forbids a node with two parents, so a revisit is a cycle or a shared node.
            if std::mem::replace(&mut seen[node.index()], true) {
                return Err(ImportError::Parse("node graph has a cycle".into()));
            }
            let xf = parent * Mat4::from_cols_array_2d(&node.transform().matrix());
            if let Some(mesh) = node.mesh() {
                visit(mesh, xf, &mut sets)?;
            }
            let kids: Vec<_> = node.children().map(|c| (c, xf)).collect();
            stack.extend(kids.into_iter().rev());
        }
    } else {
        for mesh in doc.meshes() {
            visit(mesh, Mat4::IDENTITY, &mut sets)?;
        }
    }
    debug_assert!(sets.len() >= material_count);
    // Say what was left out, as the USD importer does.
    if lines > 0 {
        notes.push(format!("{lines} line part(s) left out: only triangles can be painted"));
    }
    if no_positions > 0 {
        notes.push(format!("{no_positions} part(s) without vertex positions left out"));
    }
    finish(Model::new(submeshes, sets, points), generated, notes)
}

// ---------------------------------------------------------------------------
// OBJ
// ---------------------------------------------------------------------------

/// Load an `.obj` file; its `mtllib` and textures resolve next to it.
pub fn load_obj(path: &Path, opts: &ImportOptions) -> Result<ImportedModel, ImportError> {
    let bytes = std::fs::read(path).map_err(|e| ImportError::Io(e.to_string()))?;
    // Older Windows tools write names in Latin-1 ("Zürich"): read those bytes as Latin-1 rather
    // than refuse the file.
    let text = String::from_utf8(bytes).unwrap_or_else(|e| e.into_bytes().iter().map(|&b| char::from(b)).collect());
    load_obj_impl(&text, None, path.parent(), opts)
}

/// Load OBJ text. `mtl` is the material library text, if any; textures are not loaded.
pub fn load_obj_str(
    obj: &str,
    mtl: Option<&str>,
    opts: &ImportOptions,
) -> Result<ImportedModel, ImportError> {
    load_obj_impl(obj, mtl, None, opts)
}

/// A glTF URI that is data, or a relative path that stays in the model's folder.
fn uri_stays_inside(uri: &str) -> bool {
    if uri.starts_with("data:") {
        return true;
    }
    let lower = uri.to_ascii_lowercase();
    !uri.contains("://") && !lower.contains("%2e%2e") && !lower.contains("%2f") && !lower.contains("%5c") && relative_inside(uri)
}

/// A relative path with no `..`, root or drive: it cannot leave the folder it is joined to.
fn relative_inside(name: &str) -> bool {
    let name = name.replace('\\', "/");
    let path = Path::new(&name);
    !name.starts_with('/') && path.components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

/// The largest material library or texture file an OBJ may name.
const MAX_SIDE_FILE_BYTES: u64 = 512 << 20;

/// A file named by an OBJ, next to it: only a regular file inside `base`, up to
/// [`MAX_SIDE_FILE_BYTES`].
fn side_file(base: &Path, name: &str) -> Option<PathBuf> {
    if !relative_inside(name) {
        return None;
    }
    let path = base.join(name.replace('\\', "/"));
    let meta = std::fs::metadata(&path).ok()?;
    (meta.is_file() && meta.len() <= MAX_SIDE_FILE_BYTES).then_some(path)
}

/// The file name of an MTL texture statement without its options (`-bm 1.0 normal.png` and
/// `-s 2 2 1 tile.png` name `normal.png` and `tile.png`). The name may hold spaces.
fn texture_file_name(statement: &str) -> &str {
    // Each option and how many values follow it.
    const OPTIONS: &[(&str, usize)] = &[
        ("-blendu", 1), ("-blendv", 1), ("-bm", 1), ("-boost", 1), ("-cc", 1), ("-clamp", 1),
        ("-imfchan", 1), ("-texres", 1), ("-type", 1), ("-mm", 2), ("-o", 3), ("-s", 3), ("-t", 3),
    ];
    let mut rest = statement.trim();
    loop {
        let Some((head, tail)) = rest.split_once(char::is_whitespace) else { return rest };
        let Some(&(_, args)) = OPTIONS.iter().find(|(o, _)| o.eq_ignore_ascii_case(head)) else { return rest };
        rest = tail.trim_start();
        for i in 0..args {
            // `-o`, `-s` and `-t` take one to three numbers: stop at the first word that is not one.
            let Some((value, after)) = rest.split_once(char::is_whitespace) else { return rest };
            if i > 0 && args == 3 && value.parse::<f32>().is_err() {
                break;
            }
            rest = after.trim_start();
        }
    }
}

fn read_texture(base: Option<&Path>, name: &str, notes: &mut Vec<String>) -> Option<Img> {
    let base = base?;
    let name = texture_file_name(name);
    let Some(path) = side_file(base, name) else {
        notes.push(format!("texture {name} is not a file next to the model; skipped"));
        return None;
    };
    let decoded = image::ImageReader::open(&path)
        .and_then(|r| r.with_guessed_format())
        .map_err(image::ImageError::IoError)
        .and_then(decode_texture);
    match decoded {
        Ok(i) => {
            let i = i.to_rgba8();
            Some(Img {
                w: i.width() as usize,
                h: i.height() as usize,
                rgba: i.into_raw(),
            })
        }
        Err(e) => {
            notes.push(format!("texture {name} could not be read: {e}"));
            None
        }
    }
}

fn load_obj_impl(
    obj: &str,
    mtl: Option<&str>,
    base: Option<&Path>,
    opts: &ImportOptions,
) -> Result<ImportedModel, ImportError> {
    let mut notes = Vec::new();
    let has_faces = obj
        .lines()
        .any(|l| l.starts_with("f ") || l.starts_with("f\t"));
    let vertex_lines = obj.lines().filter(|l| l.starts_with("v ")).count();
    let lo = tobj::LoadOptions {
        triangulate: true,
        single_index: true,
        ..Default::default()
    };
    let loader = |p: &Path| -> tobj::MTLLoadResult {
        if let Some(text) = mtl {
            return tobj::load_mtl_buf(&mut Cursor::new(text.as_bytes()));
        }
        let file = base.and_then(|b| side_file(b, &p.to_string_lossy()));
        match file.map(std::fs::read_to_string) {
            Some(Ok(text)) => tobj::load_mtl_buf(&mut Cursor::new(text.into_bytes())),
            _ => Err(tobj::LoadError::OpenFileFailed),
        }
    };
    let (models, materials) = tobj::load_obj_buf(&mut Cursor::new(obj.as_bytes()), &lo, loader)
        .map_err(|e| ImportError::Parse(e.to_string()))?;
    let materials = match materials {
        Ok(m) => m,
        Err(e) => {
            if obj.lines().any(|l| l.trim_start().starts_with("mtllib")) {
                notes.push(format!("the material library could not be read ({e}); the model uses a plain material"));
            }
            Vec::new()
        }
    };

    let mut sets: Vec<TextureSet> = materials
        .iter()
        .map(|m| {
            let unknown = |k: &str| m.unknown_param.get(k).map(String::as_str);
            let num =
                |k: &str| unknown(k).and_then(|s| s.split_whitespace().next()?.parse::<f32>().ok());
            let d = m.diffuse.unwrap_or([0.8, 0.8, 0.8]);
            let shininess = m.shininess.unwrap_or(0.0);
            let src = MaterialSources {
                name: m.name.clone(),
                colour_factor: [d[0], d[1], d[2], m.dissolve.unwrap_or(1.0)],
                roughness_factor: num("Pr").unwrap_or_else(|| (2.0 / (shininess + 2.0)).powf(0.25)),
                metallic_factor: num("Pm").unwrap_or(0.0),
                colour: m
                    .diffuse_texture
                    .as_deref()
                    .and_then(|t| read_texture(base, t, &mut notes)),
                roughness: unknown("map_Pr")
                    .and_then(|t| read_texture(base, t, &mut notes))
                    .map(|i| (i, 0)),
                metallic: unknown("map_Pm")
                    .and_then(|t| read_texture(base, t, &mut notes))
                    .map(|i| (i, 0)),
                normal: m
                    .normal_texture
                    .as_deref()
                    .and_then(|t| read_texture(base, t, &mut notes)),
                ao: m
                    .ambient_texture
                    .as_deref()
                    .and_then(|t| read_texture(base, t, &mut notes))
                    .map(|i| (i, 0)),
                // OBJ has no opaque flag: its `d` and texture alpha are coverage.
                opaque: false,
            };
            assemble(src, opts.default_texture_size)
        })
        .collect();
    let mut default_index: Option<usize> = None;

    let mut submeshes = Vec::new();
    let mut generated = false;
    for m in &models {
        let mesh = &m.mesh;
        let positions: Vec<Vec3> = mesh
            .positions
            .chunks_exact(3)
            .map(|p| Vec3::new(p[0], p[1], p[2]))
            .collect();
        let normals: Vec<Vec3> = mesh
            .normals
            .chunks_exact(3)
            .map(|p| Vec3::new(p[0], p[1], p[2]))
            .collect();
        // OBJ v grows upward; flip to the top-left convention.
        let uvs: Vec<Vec2> = mesh
            .texcoords
            .chunks_exact(2)
            .map(|t| Vec2::new(t[0], 1.0 - t[1]))
            .collect();
        if mesh.indices.iter().any(|&i| i as usize >= positions.len())
            || (!uvs.is_empty() && uvs.len() != positions.len())
            || (!normals.is_empty() && normals.len() != positions.len())
        {
            return Err(ImportError::Parse("OBJ index out of range".into()));
        }
        let material = match mesh.material_id.filter(|&i| i < sets.len()) {
            Some(i) => i,
            None => *default_index.get_or_insert_with(|| {
                sets.push(default_set(opts));
                sets.len() - 1
            }),
        };
        let mut sm = SubMesh {
            name: m.name.clone(),
            positions,
            normals,
            uvs,
            indices: mesh.indices.clone(),
            material,
        };
        if sm.normals.is_empty() && sm.triangle_count() > 0 {
            sm.generate_flat_normals();
            generated = true;
        }
        submeshes.push(sm);
    }
    let points = if has_faces { 0 } else { vertex_lines };
    finish(Model::new(submeshes, sets, points), generated, notes)
}

#[cfg(test)]
mod mtl_tests {
    use super::texture_file_name;

    #[test]
    fn texture_options_are_not_part_of_the_file_name() {
        assert_eq!(texture_file_name("normal.png"), "normal.png");
        assert_eq!(texture_file_name("-bm 1.000000 normal.png"), "normal.png");
        assert_eq!(texture_file_name("-s 2 2 1 tile.png"), "tile.png");
        assert_eq!(texture_file_name("-s 2 2 my tile.png"), "my tile.png");
        assert_eq!(texture_file_name("-blendu on -clamp off wood grain.jpg"), "wood grain.jpg");
    }
}
