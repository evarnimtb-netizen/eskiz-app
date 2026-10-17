//! USD import: `.usdz`, `.usda` and `.usdc` into a [`Model`].
//!
//! The reader is the `openusd` crate used at the layer level (no composition engine): the root layer of a package,
//! or a single file, is walked prim by prim. That keeps the behaviour predictable, so features that need
//! composition (references, payloads, sublayers, instancing) are reported as unsupported instead of half-working.
//!
//! Supported: `Mesh` (points, faceVertexCounts, faceVertexIndices; polygons are fan-triangulated), normals
//! (`normals` or `primvars:normals`), `primvars:st` (or the primvar named by the material's `UsdPrimvarReader_float2`)
//! with `vertex`, `faceVarying`, `uniform` and `constant` interpolation and indexed primvars, `xformOp` transforms
//! baked into points (nested `Xform`s; negative determinants flip the winding), `GeomSubset` material splits, and
//! `UsdPreviewSurface` materials with `UsdUVTexture` inputs (diffuseColor, roughness, metallic, occlusion, normal,
//! opacity). Textures are read from inside the USDZ (or next to a loose file) as PNG or JPEG.
//!
//! Eskiz's UV origin is the top-left texel corner, USD's `st` origin is bottom-left, so `v = 1 - t` (the exporter
//! writes `t = 1 - v`).

use std::collections::HashMap;
use std::path::Path as FsPath;

use glam::{Mat3, Mat4, Quat, Vec2, Vec3};
use openusd::sdf::{AbstractData, Path, Specifier, Value};
use openusd::usdz::Archive;
use openusd::{usda, usdc};

use crate::load::{assemble, finish, Img, ImportError, ImportOptions, ImportedModel, MaterialSources};
use crate::model::{Model, SubMesh, TextureSet};

fn unsupported(what: &str) -> ImportError {
    ImportError::Unsupported(format!(
        "This USD file uses {what}, which Eskiz cannot import yet. Export the model as GLB, glTF or OBJ from the \
         app that made it, or flatten it to plain meshes first."
    ))
}

fn bad(msg: impl Into<String>) -> ImportError {
    ImportError::Parse(msg.into())
}

// ---------------------------------------------------------------------------
// Value helpers
// ---------------------------------------------------------------------------

fn str_of(v: &Value) -> Option<&str> {
    match v {
        Value::String(s) => Some(s),
        Value::Token(t) => Some(t.as_str()),
        _ => None,
    }
}

fn strings_of(v: &Value) -> Vec<String> {
    match v {
        Value::TokenVec(t) => t.iter().map(|t| t.as_str().to_owned()).collect(),
        Value::StringVec(t) => t.clone(),
        _ => Vec::new(),
    }
}

fn paths_of(v: &Value) -> Vec<String> {
    match v {
        Value::PathListOp(l) => l.flatten().iter().map(ToString::to_string).collect(),
        Value::PathVec(p) => p.iter().map(ToString::to_string).collect(),
        _ => Vec::new(),
    }
}

fn num_of(v: &Value) -> Option<f32> {
    match v {
        Value::Float(f) => Some(*f),
        Value::Double(f) => Some(*f as f32),
        Value::Half(h) => Some(h.to_f32()),
        Value::Int(i) => Some(*i as f32),
        _ => None,
    }
}

fn vec3_of(v: &Value) -> Option<Vec3> {
    match v {
        Value::Vec3f(a) => Some(Vec3::new(a.x, a.y, a.z)),
        Value::Vec3d(a) => Some(Vec3::new(a.x as f32, a.y as f32, a.z as f32)),
        Value::Vec3h(a) => Some(Vec3::new(a.x.to_f32(), a.y.to_f32(), a.z.to_f32())),
        _ => None,
    }
}

fn vec3s_of(v: &Value) -> Option<Vec<Vec3>> {
    match v {
        Value::Vec3fVec(a) => Some(a.iter().map(|a| Vec3::new(a.x, a.y, a.z)).collect()),
        Value::Vec3dVec(a) => Some(a.iter().map(|a| Vec3::new(a.x as f32, a.y as f32, a.z as f32)).collect()),
        Value::Vec3hVec(a) => Some(
            a.iter()
                .map(|a| Vec3::new(a.x.to_f32(), a.y.to_f32(), a.z.to_f32()))
                .collect(),
        ),
        _ => None,
    }
}

fn vec2s_of(v: &Value) -> Option<Vec<Vec2>> {
    match v {
        Value::Vec2fVec(a) => Some(a.iter().map(|a| Vec2::new(a.x, a.y)).collect()),
        Value::Vec2dVec(a) => Some(a.iter().map(|a| Vec2::new(a.x as f32, a.y as f32)).collect()),
        Value::Vec2hVec(a) => Some(a.iter().map(|a| Vec2::new(a.x.to_f32(), a.y.to_f32())).collect()),
        _ => None,
    }
}

fn ints_of(v: &Value) -> Option<Vec<i32>> {
    match v {
        Value::IntVec(a) => Some(a.clone()),
        _ => None,
    }
}

fn is_prim_type(t: &str, names: &[&str]) -> bool {
    names.contains(&t)
}

// ---------------------------------------------------------------------------
// Primvar-style attributes
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Interp {
    Constant,
    Uniform,
    Vertex,
    FaceVarying,
}

struct Attr<T> {
    vals: Vec<T>,
    idx: Option<Vec<i32>>,
    interp: Interp,
}

impl<T: Copy> Attr<T> {
    /// Element for a face corner, or `None` when the data is shorter than the topology needs.
    fn get(&self, corner: usize, point: usize, face: usize) -> Option<T> {
        let e = match self.interp {
            Interp::Constant => 0,
            Interp::Uniform => face,
            Interp::Vertex => point,
            Interp::FaceVarying => corner,
        };
        let e = match &self.idx {
            Some(ix) => usize::try_from(*ix.get(e)?).ok()?,
            None => e,
        };
        self.vals.get(e).copied()
    }
}

// ---------------------------------------------------------------------------
// Reader
// ---------------------------------------------------------------------------

/// Where texture files come from.
pub(crate) enum Assets<'a> {
    None,
    Package(&'a mut Archive),
    Dir(&'a FsPath),
}

struct Reader<'a> {
    d: &'a dyn AbstractData,
    assets: Assets<'a>,
    /// Directory of the root layer inside the package ("" at the top).
    layer_dir: String,
    opts: &'a ImportOptions,
    notes: Vec<String>,
    sets: Vec<TextureSet>,
    mat_cache: HashMap<String, usize>,
    img_cache: HashMap<String, Option<Img>>,
    default_index: Option<usize>,
    submeshes: Vec<SubMesh>,
    generated: bool,
    skinning_noted: bool,
    animated: bool,
}

impl<'a> Reader<'a> {
    fn note(&mut self, s: impl Into<String>) {
        let s = s.into();
        if !self.notes.contains(&s) {
            self.notes.push(s);
        }
    }

    fn raw(&self, prim: &str, field: &str) -> Option<Value> {
        let p = Path::new(prim).ok()?;
        self.d.try_field(&p, field).ok().flatten().map(std::borrow::Cow::into_owned)
    }

    fn has_spec(&self, path: &str) -> bool {
        Path::new(path).is_ok_and(|p| self.d.has_spec(&p))
    }

    /// Authored value of a property: `default`, else the first time sample.
    fn val(&mut self, prop: &str) -> Option<Value> {
        if let Some(v) = self.raw(prop, "default") {
            return Some(v);
        }
        if let Some(Value::TimeSamples(ts)) = self.raw(prop, "timeSamples") {
            if ts.len() > 1 {
                self.animated = true;
            }
            return ts.into_iter().next().map(|(_, v)| v);
        }
        None
    }

    fn type_name(&self, prim: &str) -> String {
        self.raw(prim, "typeName")
            .and_then(|v| str_of(&v).map(str::to_owned))
            .unwrap_or_default()
    }

    fn children(&self, prim: &str) -> Vec<String> {
        let names = self.raw(prim, "primChildren").map(|v| strings_of(&v)).unwrap_or_default();
        let base = if prim == "/" { "" } else { prim };
        names.into_iter().map(|n| format!("{base}/{n}")).collect()
    }

    fn rel_targets(&self, prop: &str) -> Vec<String> {
        self.raw(prop, "targetPaths").map(|v| paths_of(&v)).unwrap_or_default()
    }

    fn connections(&self, prop: &str) -> Vec<String> {
        self.raw(prop, "connectionPaths").map(|v| paths_of(&v)).unwrap_or_default()
    }

    // ---- scene walk ----

    fn walk(&mut self, prim: &str, parent: Mat4, inherited: Option<String>) -> Result<(), ImportError> {
        let spec = self.raw(prim, "specifier");
        if matches!(spec, Some(Value::Specifier(s)) if s != Specifier::Def) {
            return Ok(());
        }
        if matches!(self.raw(prim, "active"), Some(Value::Bool(false))) {
            return Ok(());
        }
        let ty = self.type_name(prim);
        if self.raw(prim, "references").is_some() || self.raw(prim, "payload").is_some() {
            return Err(unsupported("references to other layers or files"));
        }
        if matches!(self.raw(prim, "instanceable"), Some(Value::Bool(true))) || ty == "PointInstancer" {
            return Err(unsupported("instancing"));
        }
        if self.raw(prim, "inheritPaths").is_some() || self.raw(prim, "specializes").is_some() {
            return Err(unsupported("class inheritance or specializes"));
        }
        if self.raw(prim, "variantSelection").is_some() && self.raw(prim, "variantSetNames").is_some() {
            self.note("This USD file has variant sets; the authored default variant is not applied.");
        }
        if is_prim_type(&ty, &["SkelRoot", "Skeleton", "SkelAnimation", "BlendShape"]) && !self.skinning_noted {
            self.skinning_noted = true;
            self.note("Skinning, skeletons and animation are not supported; the model is imported in its rest pose.");
        }
        if matches!(self.val(&format!("{prim}.visibility")), Some(ref v) if str_of(v) == Some("invisible")) {
            return Ok(());
        }
        if matches!(self.val(&format!("{prim}.purpose")), Some(ref v) if matches!(str_of(v), Some("guide" | "proxy"))) {
            return Ok(());
        }
        if ty == "Material" || ty == "Shader" || ty == "GeomSubset" {
            return Ok(());
        }
        let xf = self.local_transform(prim, parent);
        let binding = self
            .rel_targets(&format!("{prim}.material:binding"))
            .into_iter()
            .next()
            .or(inherited);
        match ty.as_str() {
            "Mesh" => self.mesh(prim, xf, binding.as_deref())?,
            "Cube" | "Sphere" | "Cylinder" | "Cone" | "Capsule" | "Plane" | "BasisCurves" | "NurbsPatch"
            | "NurbsCurves" | "Points" | "TetMesh" | "HermiteCurves" => {
                self.note(format!("USD {ty} primitives are not supported and were skipped; only meshes are imported."));
            }
            _ => {}
        }
        for c in self.children(prim) {
            self.walk(&c, xf, binding.clone())?;
        }
        Ok(())
    }

    fn local_transform(&mut self, prim: &str, parent: Mat4) -> Mat4 {
        let Some(order) = self.val(&format!("{prim}.xformOpOrder")).map(|v| strings_of(&v)) else {
            return parent;
        };
        let mut local = Mat4::IDENTITY;
        let mut base = parent;
        for tok in order {
            if tok == "!resetXformStack!" {
                local = Mat4::IDENTITY;
                base = Mat4::IDENTITY;
                continue;
            }
            let (inv, name) = match tok.strip_prefix("!invert!") {
                Some(n) => (true, n.to_owned()),
                None => (false, tok.clone()),
            };
            let Some(value) = self.val(&format!("{prim}.{name}")) else {
                continue;
            };
            let kind = name.strip_prefix("xformOp:").unwrap_or(&name).split(':').next().unwrap_or("");
            let Some(m) = op_matrix(kind, &value) else {
                self.note(format!("Transform operation {name} is not supported and was ignored."));
                continue;
            };
            local *= if inv { m.inverse() } else { m };
        }
        base * local
    }

    // ---- meshes ----

    fn attr_vec3(&mut self, prim: &str, names: [&str; 2], n_points: usize, n_corners: usize, n_faces: usize) -> Option<Attr<Vec3>> {
        for name in names {
            let prop = format!("{prim}.{name}");
            if let Some(vals) = self.val(&prop).as_ref().and_then(vec3s_of) {
                let idx = self.val(&format!("{prop}:indices")).as_ref().and_then(ints_of);
                let interp = self.interp(&prop, if idx.is_some() { None } else { Some(vals.len()) }, n_points, n_corners, n_faces, true);
                return Some(Attr { vals, idx, interp });
            }
        }
        None
    }

    /// Interpolation from metadata, else guessed from the array length.
    fn interp(&self, prop: &str, len: Option<usize>, n_points: usize, n_corners: usize, n_faces: usize, normals: bool) -> Interp {
        if let Some(v) = self.raw(prop, "interpolation") {
            match str_of(&v) {
                Some("constant") => return Interp::Constant,
                Some("uniform") => return Interp::Uniform,
                Some("vertex" | "varying") => return Interp::Vertex,
                Some("faceVarying") => return Interp::FaceVarying,
                _ => {}
            }
        }
        match len {
            Some(l) if l == n_points => Interp::Vertex,
            Some(l) if l == n_corners => Interp::FaceVarying,
            Some(l) if l == n_faces => Interp::Uniform,
            Some(1) => Interp::Constant,
            _ if normals => Interp::Vertex,
            _ => Interp::FaceVarying,
        }
    }

    fn mesh(&mut self, prim: &str, xf: Mat4, binding: Option<&str>) -> Result<(), ImportError> {
        let name = prim.rsplit('/').next().unwrap_or("mesh").to_owned();
        let points = self
            .val(&format!("{prim}.points"))
            .as_ref()
            .and_then(vec3s_of)
            .ok_or_else(|| bad(format!("mesh {name} has no points")))?;
        let counts = self
            .val(&format!("{prim}.faceVertexCounts"))
            .as_ref()
            .and_then(ints_of)
            .ok_or_else(|| bad(format!("mesh {name} has no faceVertexCounts")))?;
        let indices = self
            .val(&format!("{prim}.faceVertexIndices"))
            .as_ref()
            .and_then(ints_of)
            .ok_or_else(|| bad(format!("mesh {name} has no faceVertexIndices")))?;
        if counts.iter().any(|&c| c < 3) {
            return Err(bad(format!("mesh {name} has a face with fewer than 3 vertices")));
        }
        let total: usize = counts.iter().map(|&c| c as usize).sum();
        if total != indices.len() {
            return Err(bad(format!(
                "mesh {name}: faceVertexCounts add up to {total} but there are {} indices",
                indices.len()
            )));
        }
        if indices.iter().any(|&i| i < 0 || i as usize >= points.len()) {
            return Err(bad(format!("mesh {name} has a vertex index out of range")));
        }
        if let Some(v) = self.val(&format!("{prim}.subdivisionScheme")) {
            if matches!(str_of(&v), Some("catmullClark" | "loop" | "bilinear")) {
                self.note("Subdivision surfaces are not supported; meshes are imported as their unsubdivided cage.");
            }
        }
        if !self.skinning_noted
            && (self.raw(&format!("{prim}.skel:joints"), "default").is_some()
                || self.has_spec(&format!("{prim}.primvars:skel:jointIndices")))
        {
            self.skinning_noted = true;
            self.note("Skinning, skeletons and animation are not supported; the model is imported in its rest pose.");
        }
        let left = self.val(&format!("{prim}.orientation")).is_some_and(|v| str_of(&v) == Some("leftHanded"));

        let n_faces = counts.len();
        let normals = self.attr_vec3(prim, ["normals", "primvars:normals"], points.len(), total, n_faces);
        let uvs = self.uv_attr(prim, binding, points.len(), total, n_faces);

        // Face -> material binding (GeomSubsets override the mesh binding).
        let mut face_bind: Vec<Option<String>> = vec![binding.map(str::to_owned); n_faces];
        for sub in self.children(prim) {
            if self.type_name(&sub) != "GeomSubset" {
                continue;
            }
            let Some(b) = self.rel_targets(&format!("{sub}.material:binding")).into_iter().next() else {
                continue;
            };
            let elem = self.val(&format!("{sub}.elementType"));
            if elem.as_ref().and_then(str_of).is_some_and(|e| e != "face") {
                continue;
            }
            let Some(ix) = self.val(&format!("{sub}.indices")).as_ref().and_then(ints_of) else {
                continue;
            };
            for f in ix {
                match usize::try_from(f).ok().filter(|&f| f < n_faces) {
                    Some(f) => face_bind[f] = Some(b.clone()),
                    None => return Err(bad(format!("mesh {name} has a GeomSubset face index out of range"))),
                }
            }
        }
        // Group faces by binding, in order of first appearance.
        let mut groups: Vec<(Option<String>, Vec<usize>)> = Vec::new();
        for (f, b) in face_bind.iter().enumerate() {
            match groups.iter_mut().find(|g| &g.0 == b) {
                Some(g) => g.1.push(f),
                None => groups.push((b.clone(), vec![f])),
            }
        }
        let mut starts = Vec::with_capacity(n_faces);
        let mut acc = 0usize;
        for &c in &counts {
            starts.push(acc);
            acc += c as usize;
        }
        let nm = Mat3::from_mat4(xf).inverse().transpose();
        let flip = (xf.determinant() < 0.0) != left;
        let multi = groups.len() > 1;
        for (gi, (bind, faces)) in groups.iter().enumerate() {
            let material = match bind {
                Some(b) => self.material_for(b),
                None => self.default_material(),
            };
            let mut map: HashMap<(u32, u32, u32), u32> = HashMap::new();
            let mut sm = SubMesh {
                name: if multi { format!("{name}.{gi}") } else { name.clone() },
                positions: Vec::new(),
                normals: Vec::new(),
                uvs: Vec::new(),
                indices: Vec::new(),
                material,
            };
            for &f in faces {
                let mut local = Vec::with_capacity(counts[f] as usize);
                for k in 0..counts[f] as usize {
                    let corner = starts[f] + k;
                    let pi = indices[corner] as usize;
                    let nrm = normals.as_ref().and_then(|a| a.get(corner, pi, f));
                    if normals.is_some() && nrm.is_none() {
                        return Err(bad(format!("mesh {name} has too few normals")));
                    }
                    let uv = match &uvs {
                        Some(a) => {
                            let u = a.get(corner, pi, f);
                            if u.is_none() {
                                return Err(bad(format!("mesh {name} has too few texture coordinates")));
                            }
                            u
                        }
                        None => None,
                    };
                    // Dedup on the exact bit patterns of the attributes.
                    let key = (
                        pi as u32,
                        nrm.map_or(u32::MAX, |n| n.to_array().iter().fold(17u32, |h, c| h.wrapping_mul(31).wrapping_add(c.to_bits()))),
                        uv.map_or(u32::MAX, |t| t.to_array().iter().fold(19u32, |h, c| h.wrapping_mul(31).wrapping_add(c.to_bits()))),
                    );
                    let vi = *map.entry(key).or_insert_with(|| {
                        sm.positions.push(xf.transform_point3(points[pi]));
                        if let Some(n) = nrm {
                            sm.normals.push((nm * n).normalize_or_zero());
                        }
                        if let Some(t) = uv {
                            sm.uvs.push(Vec2::new(t.x, 1.0 - t.y));
                        }
                        (sm.positions.len() - 1) as u32
                    });
                    local.push(vi);
                }
                for k in 1..local.len() - 1 {
                    if flip {
                        sm.indices.extend([local[0], local[k + 1], local[k]]);
                    } else {
                        sm.indices.extend([local[0], local[k], local[k + 1]]);
                    }
                }
            }
            if sm.normals.is_empty() && sm.triangle_count() > 0 {
                sm.generate_flat_normals();
                self.generated = true;
            }
            self.submeshes.push(sm);
        }
        Ok(())
    }

    fn uv_attr(&mut self, prim: &str, binding: Option<&str>, n_points: usize, n_corners: usize, n_faces: usize) -> Option<Attr<Vec2>> {
        let mut candidates: Vec<String> = Vec::new();
        if let Some(b) = binding {
            if let Some(v) = self.material_uv_name(b) {
                candidates.push(v);
            }
        }
        candidates.push("st".into());
        candidates.push("uv".into());
        candidates.push("UVMap".into());
        // Any other texCoord2f primvar.
        if let Some(props) = self.raw(prim, "propertyChildren").map(|v| strings_of(&v)) {
            for p in props {
                if let Some(n) = p.strip_prefix("primvars:") {
                    if !n.contains(':') && !candidates.iter().any(|c| c == n) {
                        candidates.push(n.to_owned());
                    }
                }
            }
        }
        for n in candidates {
            let prop = format!("{prim}.primvars:{n}");
            if let Some(vals) = self.val(&prop).as_ref().and_then(vec2s_of) {
                let idx = self.val(&format!("{prop}:indices")).as_ref().and_then(ints_of);
                let interp = self.interp(&prop, if idx.is_some() { None } else { Some(vals.len()) }, n_points, n_corners, n_faces, false);
                return Some(Attr { vals, idx, interp });
            }
        }
        None
    }

    // ---- materials ----

    fn default_material(&mut self) -> usize {
        if let Some(i) = self.default_index {
            return i;
        }
        let set = assemble(
            MaterialSources {
                name: "default".to_string(),
                colour_factor: [1.0, 1.0, 1.0, 1.0],
                roughness_factor: 1.0,
                metallic_factor: 0.0,
                ..MaterialSources::default()
            },
            self.opts.default_texture_size,
        );
        self.sets.push(set);
        let i = self.sets.len() - 1;
        self.default_index = Some(i);
        i
    }

    fn material_for(&mut self, mat: &str) -> usize {
        if let Some(&i) = self.mat_cache.get(mat) {
            return i;
        }
        let src = self.material_sources(mat);
        let i = match src {
            Some(src) => {
                self.sets.push(assemble(src, self.opts.default_texture_size));
                self.sets.len() - 1
            }
            None => self.default_material(),
        };
        self.mat_cache.insert(mat.to_owned(), i);
        i
    }

    fn surface_shader(&mut self, mat: &str) -> Option<String> {
        if let Some(t) = self.connections(&format!("{mat}.outputs:surface")).into_iter().next() {
            return Some(t.split('.').next().unwrap_or(&t).to_owned());
        }
        self.children(mat).into_iter().find(|c| self.type_name(c) == "Shader")
    }

    fn shader_id(&mut self, shader: &str) -> String {
        self.val(&format!("{shader}.info:id")).and_then(|v| str_of(&v).map(str::to_owned)).unwrap_or_default()
    }

    fn material_uv_name(&mut self, mat: &str) -> Option<String> {
        let shader = self.surface_shader(mat)?;
        let tex = match self.input(&shader, "diffuseColor", 0) {
            Input::Tex { shader, .. } => shader,
            _ => return None,
        };
        let st = self.connections(&format!("{tex}.inputs:st")).into_iter().next()?;
        let reader = st.split('.').next()?.to_owned();
        self.val(&format!("{reader}.inputs:varname")).and_then(|v| str_of(&v).map(str::to_owned))
    }

    fn input(&mut self, shader: &str, name: &str, depth: u8) -> Input {
        let prop = format!("{shader}.inputs:{name}");
        if depth > 4 {
            return Input::None;
        }
        if let Some(target) = self.connections(&prop).into_iter().next() {
            let (prim, out) = target.split_once('.').unwrap_or((&target, ""));
            let out = out.strip_prefix("outputs:").unwrap_or(out);
            match self.type_name(prim).as_str() {
                "Shader" => {
                    let id = self.shader_id(prim);
                    if id == "UsdUVTexture" {
                        let channel = match out {
                            "r" => Some(0),
                            "g" => Some(1),
                            "b" => Some(2),
                            "a" => Some(3),
                            _ => None,
                        };
                        return Input::Tex { shader: prim.to_owned(), channel };
                    }
                    return Input::Other(id);
                }
                _ => {
                    // A material interface input: follow it.
                    let iface = target.split_once('.').map(|(p, o)| (p.to_owned(), o.to_owned()));
                    if let Some((p, o)) = iface {
                        let r = self.input_at(&format!("{p}.{o}"), depth + 1);
                        return r;
                    }
                }
            }
        }
        match self.val(&prop) {
            Some(v) => Input::Const(v),
            None => Input::None,
        }
    }

    fn input_at(&mut self, prop: &str, depth: u8) -> Input {
        if depth > 4 {
            return Input::None;
        }
        if let Some(target) = self.connections(prop).into_iter().next() {
            let (prim, out) = target.split_once('.').unwrap_or((&target, ""));
            let out = out.strip_prefix("outputs:").unwrap_or(out);
            if self.type_name(prim) == "Shader" && self.shader_id(prim) == "UsdUVTexture" {
                let channel = match out {
                    "r" => Some(0),
                    "g" => Some(1),
                    "b" => Some(2),
                    "a" => Some(3),
                    _ => None,
                };
                return Input::Tex { shader: prim.to_owned(), channel };
            }
            return self.input_at(&target, depth + 1);
        }
        match self.val(prop) {
            Some(v) => Input::Const(v),
            None => Input::None,
        }
    }

    fn material_sources(&mut self, mat: &str) -> Option<MaterialSources> {
        let name = mat.rsplit('/').next().unwrap_or("material").to_owned();
        let Some(shader) = self.surface_shader(mat) else {
            self.note(format!("Material {name} has no surface shader; a plain material is used."));
            return None;
        };
        let id = self.shader_id(&shader);
        if id != "UsdPreviewSurface" {
            self.note(format!(
                "Material {name} uses the shader \"{id}\", which is not supported (only UsdPreviewSurface is); a plain material is used."
            ));
            return None;
        }
        let mut src = MaterialSources {
            name,
            colour_factor: [0.18, 0.18, 0.18, 1.0],
            roughness_factor: 0.5,
            metallic_factor: 0.0,
            ..MaterialSources::default()
        };
        match self.input(&shader, "diffuseColor", 0) {
            Input::Const(v) => {
                if let Some(c) = vec3_of(&v) {
                    src.colour_factor = [c.x, c.y, c.z, 1.0];
                }
            }
            Input::Tex { shader: t, .. } => {
                src.colour_factor = [1.0, 1.0, 1.0, 1.0];
                src.colour = self.texture(&t);
            }
            _ => {}
        }
        match self.input(&shader, "opacity", 0) {
            Input::Const(v) => {
                if let Some(o) = num_of(&v) {
                    src.colour_factor[3] = o;
                }
            }
            Input::Tex { shader: t, channel } => {
                // Opacity is read from the colour image's alpha only when both come from the same texture.
                let same = matches!(self.input(&shader, "diffuseColor", 0), Input::Tex { shader: ref c, .. } if *c == t);
                if !(same && channel == Some(3)) {
                    self.note(format!("Material {}: a separate opacity texture is not supported and was ignored.", src.name));
                }
            }
            _ => {}
        }
        for (input, is_rough) in [("roughness", true), ("metallic", false)] {
            match self.input(&shader, input, 0) {
                Input::Const(v) => {
                    if let Some(f) = num_of(&v) {
                        if is_rough {
                            src.roughness_factor = f;
                        } else {
                            src.metallic_factor = f;
                        }
                    }
                }
                Input::Tex { shader: t, channel } => {
                    let img = self.texture(&t).map(|i| (i, channel.unwrap_or(0)));
                    if is_rough {
                        src.roughness_factor = 1.0;
                        src.roughness = img;
                    } else {
                        src.metallic_factor = 1.0;
                        src.metallic = img;
                    }
                }
                _ => {}
            }
        }
        if let Input::Tex { shader: t, channel } = self.input(&shader, "occlusion", 0) {
            src.ao = self.texture(&t).map(|i| (i, channel.unwrap_or(0)));
        }
        if let Input::Tex { shader: t, .. } = self.input(&shader, "normal", 0) {
            src.normal = self.texture(&t);
        }
        Some(src)
    }

    // ---- textures ----

    fn texture(&mut self, tex_shader: &str) -> Option<Img> {
        let file = match self.val(&format!("{tex_shader}.inputs:file")) {
            Some(Value::AssetPath(a)) => a.authored_path,
            _ => return None,
        };
        if let Some(c) = self.img_cache.get(&file) {
            return c.clone();
        }
        let img = self.load_image(&file);
        self.img_cache.insert(file, img.clone());
        img
    }

    fn load_image(&mut self, authored: &str) -> Option<Img> {
        let ext = authored.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        if !matches!(ext.as_str(), "png" | "jpg" | "jpeg") {
            self.note(format!("Texture {authored} is a .{ext} image; only PNG and JPEG textures are supported, so it was skipped."));
            return None;
        }
        let rel = authored.trim_start_matches("./").replace('\\', "/");
        let bytes = match &mut self.assets {
            Assets::None => {
                self.note(format!("Texture {authored} could not be loaded because the file was opened without its folder."));
                return None;
            }
            Assets::Package(ar) => {
                let joined = normalize(&format!("{}{}", self.layer_dir, rel));
                [joined, rel.clone()]
                    .into_iter()
                    .find(|n| ar.contains(n))
                    .and_then(|n| ar.entry(&n).ok().map(|b| b.to_vec()))
            }
            Assets::Dir(dir) => {
                if rel.starts_with('/') || rel.contains("://") {
                    None
                } else {
                    std::fs::read(dir.join(normalize(&rel))).ok()
                }
            }
        };
        let Some(bytes) = bytes else {
            self.note(format!("Texture {authored} was not found in the file and was skipped."));
            return None;
        };
        match image::load_from_memory(&bytes) {
            Ok(i) => {
                let i = i.to_rgba8();
                Some(Img { w: i.width() as usize, h: i.height() as usize, rgba: i.into_raw() })
            }
            Err(e) => {
                self.note(format!("Texture {authored} could not be decoded: {e}"));
                None
            }
        }
    }
}

enum Input {
    None,
    Const(Value),
    Tex { shader: String, channel: Option<usize> },
    #[allow(dead_code)]
    Other(String),
}

/// Collapse `.` and `..` segments in a relative path.
fn normalize(p: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            s => out.push(s),
        }
    }
    out.join("/")
}

fn rot(axis: char, deg: f32) -> Mat4 {
    let r = deg.to_radians();
    match axis {
        'X' => Mat4::from_rotation_x(r),
        'Y' => Mat4::from_rotation_y(r),
        _ => Mat4::from_rotation_z(r),
    }
}

/// Matrix (column-vector convention) for one xformOp.
fn op_matrix(kind: &str, v: &Value) -> Option<Mat4> {
    match kind {
        "translate" => vec3_of(v).map(Mat4::from_translation),
        "scale" => vec3_of(v).map(Mat4::from_scale),
        "rotateX" | "rotateY" | "rotateZ" => num_of(v).map(|d| rot(kind.chars().last().unwrap_or('X'), d)),
        "orient" => {
            let q = match v {
                Value::Quatf(q) => Quat::from_xyzw(q.x, q.y, q.z, q.w),
                Value::Quatd(q) => Quat::from_xyzw(q.x as f32, q.y as f32, q.z as f32, q.w as f32),
                Value::Quath(q) => Quat::from_xyzw(q.x.to_f32(), q.y.to_f32(), q.z.to_f32(), q.w.to_f32()),
                _ => return None,
            };
            Some(Mat4::from_quat(q.normalize()))
        }
        "transform" => match v {
            // Row-major with the translation in the last row == column-major of the transpose.
            Value::Matrix4d(m) => {
                let a: [f32; 16] = std::array::from_fn(|i| m.0[i] as f32);
                Some(Mat4::from_cols_array(&a))
            }
            _ => None,
        },
        k if k.len() == 9 && k.starts_with("rotate") => {
            // rotateXYZ: X is applied first, so the column-form matrix is Rz * Ry * Rx.
            let d = vec3_of(v)?;
            let axes: Vec<char> = k[6..].chars().collect();
            let deg = |c: char| match c {
                'X' => d.x,
                'Y' => d.y,
                _ => d.z,
            };
            if !axes.iter().all(|c| matches!(c, 'X' | 'Y' | 'Z')) {
                return None;
            }
            Some(rot(axes[2], deg(axes[2])) * rot(axes[1], deg(axes[1])) * rot(axes[0], deg(axes[0])))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------

fn import_layer(
    d: &dyn AbstractData,
    assets: Assets<'_>,
    layer_dir: String,
    opts: &ImportOptions,
) -> Result<ImportedModel, ImportError> {
    let mut r = Reader {
        d,
        assets,
        layer_dir,
        opts,
        notes: Vec::new(),
        sets: Vec::new(),
        mat_cache: HashMap::new(),
        img_cache: HashMap::new(),
        default_index: None,
        submeshes: Vec::new(),
        generated: false,
        skinning_noted: false,
        animated: false,
    };
    if r.raw("/", "subLayers").is_some_and(|v| !matches!(&v, Value::StringVec(s) if s.is_empty())) {
        return Err(unsupported("sublayers (other layers or files)"));
    }
    let up_z = r.raw("/", "upAxis").is_some_and(|v| str_of(&v) == Some("Z"));
    let (start, end) = (
        r.raw("/", "startTimeCode").as_ref().and_then(num_of),
        r.raw("/", "endTimeCode").as_ref().and_then(num_of),
    );
    if matches!((start, end), (Some(s), Some(e)) if e > s) {
        r.animated = true;
    }
    // Z-up scenes are rotated to Eskiz's Y-up: (x, y, z) -> (x, z, -y).
    let root = if up_z { Mat4::from_rotation_x(-std::f32::consts::FRAC_PI_2) } else { Mat4::IDENTITY };
    for p in r.children("/") {
        r.walk(&p, root, None)?;
    }
    if r.animated {
        r.note("Animation is not supported; the first frame (or default pose) of the model was imported.");
    }
    let points = 0;
    let (submeshes, sets, generated, notes) = (r.submeshes, r.sets, r.generated, r.notes);
    finish(Model::new(submeshes, sets, points), generated, notes)
}

/// Load a `.usdz` package from memory. Textures are read from inside the package.
pub fn load_usdz_slice(bytes: &[u8], opts: &ImportOptions) -> Result<ImportedModel, ImportError> {
    let mut ar = Archive::from_bytes(bytes.to_vec())
        .map_err(|e| bad(format!("this is not a valid USDZ package: {e}")))?;
    let first = ar
        .first_layer_name()
        .ok_or_else(|| bad("this USDZ package contains no USD layer"))?;
    let data = ar
        .read_first_layer()
        .map_err(|e| bad(format!("the USDZ root layer could not be read: {e}")))?;
    let dir = first.rsplit_once('/').map_or(String::new(), |(d, _)| format!("{d}/"));
    import_layer(&*data, Assets::Package(&mut ar), dir, opts)
}

fn read_loose(bytes: &[u8]) -> Result<Box<dyn AbstractData>, ImportError> {
    if bytes.starts_with(b"PXR-USDC") {
        let d = usdc::CrateData::open(bytes.to_vec(), true)
            .map_err(|e| bad(format!("the USD crate (.usdc) file could not be read: {e}")))?;
        return Ok(Box::new(d));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| bad("this file is not a USD text (.usda) or crate (.usdc) file"))?;
    if !text.trim_start().starts_with("#usda") {
        return Err(bad("this file is not a USD text (.usda) or crate (.usdc) file"));
    }
    let d = usda::parse(text).map_err(|e| bad(format!("the USDA file could not be parsed: {e}")))?;
    Ok(Box::new(d))
}

/// Load USD bytes of any flavour (`.usdz`, `.usdc`, `.usda`, detected from the content). A loose layer has no
/// folder to read textures from, so its textures are reported as skipped.
pub fn load_usd_slice(bytes: &[u8], opts: &ImportOptions) -> Result<ImportedModel, ImportError> {
    if bytes.starts_with(b"PK") {
        return load_usdz_slice(bytes, opts);
    }
    let d = read_loose(bytes)?;
    import_layer(&*d, Assets::None, String::new(), opts)
}

/// Load a `.usdz`, `.usdc`, `.usda` or `.usd` file. Textures of a loose layer resolve next to it.
pub fn load_usd_file(path: &FsPath, opts: &ImportOptions) -> Result<ImportedModel, ImportError> {
    let bytes = std::fs::read(path).map_err(|e| ImportError::Io(e.to_string()))?;
    if bytes.starts_with(b"PK") {
        return load_usdz_slice(&bytes, opts);
    }
    let d = read_loose(&bytes)?;
    let dir = path.parent().unwrap_or_else(|| FsPath::new("."));
    import_layer(&*d, Assets::Dir(dir), String::new(), opts)
}
