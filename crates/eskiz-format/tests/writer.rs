//! Writer tests: archive and document round trips, thumbnails, atomic writes and validation.
//! Documents are read back with the crate's own reader. Codec tests live in `src/codecs.rs`.

use std::env;
use std::fs;
use std::io::{Cursor, Read};

use eskiz_format::document::{self, DocumentBuilder, GroupSpec, LayerSpec, Node};
use eskiz_format::keyedarchive::{self, NsNumber, NsObject, NsValue, Object};
use eskiz_format::{blend_mode_name, Document, Error, Item};

const W: u32 = 1000;
const H: u32 = 700;

/// Premultiplied RGBA with gradients and noise. The bottom-right area is fully transparent, so
/// some whole tiles are skipped at tile size 256.
fn pattern(seed: u32, w: usize, h: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(w * h * 4);
    let mut state = seed.wrapping_mul(2_654_435_761).wrapping_add(12_345);
    for y in 0..h {
        for x in 0..w {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let a = if x >= 512 && y >= 256 {
                0
            } else {
                ((x * 7 + y * 3) as u32 + seed) as u8
            };
            let noise = (state >> 8) as u8;
            let r = noise.min(a);
            let g = ((x + y) as u8).min(a);
            let b = ((x as u8) ^ (y as u8)).min(a);
            out.extend_from_slice(&[r, g, b, a]);
        }
    }
    out
}

fn solid(rgba: [u8; 4], w: usize, h: usize) -> Vec<u8> {
    rgba.iter().cycle().take(w * h * 4).copied().collect()
}

/// Groups, hidden and locked layers, several blend modes, an empty layer and an odd canvas.
fn sample_builder() -> DocumentBuilder {
    let (w, h) = (W as usize, H as usize);
    let mut b = DocumentBuilder::new(W, H);
    b.name = "Odd canvas".into();
    b.author = Some("Test Author".into());
    b.color_profile = Some("sRGB IEC61966-2.1".into());
    b.background_color = [0.1, 0.2, 0.3, 1.0];
    b.orientation = 2;
    b.dpi = 264.0;
    b.tracked_time = 1234.5;
    b.stroke_count = 321;
    b.tree = vec![
        Node::Group(GroupSpec {
            name: "Top group".into(),
            opacity: 0.5,
            collapsed: true,
            children: vec![
                Node::Layer(LayerSpec {
                    name: "Ink".into(),
                    opacity: 0.8,
                    blend_mode: 1,
                    locked: true,
                    pixels: Some(pattern(1, w, h)),
                    ..Default::default()
                }),
                Node::Layer(LayerSpec {
                    name: "Hidden paint".into(),
                    hidden: true,
                    clipped: true,
                    blend_mode: 26,
                    pixels: Some(pattern(2, w, h)),
                    ..Default::default()
                }),
            ],
            ..Default::default()
        }),
        Node::Layer(LayerSpec {
            name: "Transparency locked".into(),
            alpha_locked: true,
            private: true,
            blend_mode: 34,
            pixels: Some(pattern(3, w, h)),
            ..Default::default()
        }),
        Node::Layer(LayerSpec {
            name: "Empty".into(),
            uuid: Some("EMPTY-0".into()),
            ..Default::default()
        }),
    ];
    b
}

fn flat_layers<'a>(nodes: &'a [Node], out: &mut Vec<&'a LayerSpec>) {
    for node in nodes {
        match node {
            Node::Layer(layer) => out.push(layer),
            Node::Group(group) => flat_layers(&group.children, out),
        }
    }
}

fn reopen(builder: &DocumentBuilder) -> Document<Cursor<Vec<u8>>> {
    let bytes = builder.to_bytes().expect("document writes");
    Document::from_reader(Cursor::new(bytes)).expect("document reads back")
}

fn archive_bytes(package: &[u8]) -> Vec<u8> {
    let mut zip = zip::ZipArchive::new(Cursor::new(package)).unwrap();
    let mut file = zip.by_name("Document.archive").unwrap();
    let mut out = Vec::new();
    file.read_to_end(&mut out).unwrap();
    out
}

/// The first top-level item of `Document.archive`, as decoded by the keyed-archive reader.
fn first_top_item(package: &[u8]) -> Object {
    let root = keyedarchive::unarchive(&archive_bytes(package)).unwrap();
    root.as_archived().unwrap().array("unwrappedLayers").unwrap()[0].clone()
}

#[test]
fn keyed_archive_nested_roundtrip() {
    let profile = NsObject::new("ValkyrieColorProfile")
        .field("SiColorProfileArchiveICCNameKey", NsValue::String("sRGB".into()));
    let value = NsValue::Object(
        NsObject::new("SilicaDocument")
            .field("name", NsValue::String("nested".into()))
            .field("count", NsValue::Int(-7))
            .field("ratio", NsValue::Real(0.25))
            .field("flag", NsValue::Bool(true))
            .field("bytes", NsValue::Data(vec![1, 2, 3]))
            .field("empty", NsValue::Null)
            .field("seven", NsValue::Number(NsNumber::Int(7)))
            .field(
                "list",
                NsValue::Array {
                    items: vec![
                        NsValue::String("a".into()),
                        NsValue::Object(profile.clone()),
                        NsValue::Array {
                            items: vec![NsValue::Int(1)],
                            mutable: false,
                        },
                    ],
                    mutable: true,
                },
            )
            .field(
                "dict",
                NsValue::Dict {
                    entries: vec![
                        ("k".into(), NsValue::Real(2.5)),
                        ("obj".into(), NsValue::Object(profile)),
                    ],
                    mutable: true,
                },
            ),
    );
    let bytes = keyedarchive::write(&value).unwrap();
    assert!(bytes.starts_with(b"bplist00"));

    let root = keyedarchive::unarchive(&bytes).unwrap();
    let doc = root.as_archived().unwrap();
    assert_eq!(doc.class_name, "SilicaDocument");
    assert_eq!(doc.class_hierarchy, ["SilicaDocument", "NSObject"]);
    assert_eq!(doc.str("name"), Some("nested"));
    assert_eq!(doc.i64("count"), Some(-7));
    assert_eq!(doc.f64("ratio"), Some(0.25));
    assert!(doc.truthy("flag"));
    assert_eq!(doc.data("bytes"), Some(&[1u8, 2, 3][..]));
    assert!(doc.get("empty").unwrap().is_null());

    let seven = doc.archived("seven").unwrap();
    assert_eq!(seven.class_name, "NSNumber");
    assert_eq!(seven.i64("NS.intval"), Some(7));

    let list = doc.array("list").unwrap();
    assert_eq!(list[0].as_str(), Some("a"));
    let child = list[1].as_archived().unwrap();
    assert_eq!(child.class_name, "ValkyrieColorProfile");
    assert_eq!(child.str("SiColorProfileArchiveICCNameKey"), Some("sRGB"));
    assert_eq!(list[2].as_array().unwrap()[0].as_i64(), Some(1));

    let Object::Dict(pairs) = doc.get("dict").unwrap() else {
        panic!("dict should decode to pairs");
    };
    assert_eq!(pairs[0].0.as_str(), Some("k"));
    assert_eq!(pairs[0].1.as_f64(), Some(2.5));
    assert_eq!(
        pairs[1].1.as_archived().unwrap().str("SiColorProfileArchiveICCNameKey"),
        Some("sRGB")
    );
}

#[test]
fn full_document_roundtrip_1000x700() {
    let builder = sample_builder();
    let mut doc = reopen(&builder);
    assert_eq!(doc.name.as_deref(), Some("Odd canvas"));
    assert_eq!((doc.width, doc.height, doc.tile_size), (W, H, 256));
    assert_eq!(doc.orientation, Some(2));
    assert_eq!(doc.stroke_count, Some(321));
    assert_eq!(doc.author.as_deref(), Some("Test Author"));
    assert_eq!(doc.color_profile.as_deref(), Some("sRGB IEC61966-2.1"));
    assert!((doc.dpi.unwrap() - 264.0).abs() < 1e-9);
    assert!((doc.tracked_time.unwrap() - 1234.5).abs() < 1e-9);
    assert_eq!(doc.background_color, Some([0.1f32, 0.2, 0.3, 1.0]));
    assert!(!doc.background_hidden);

    let [Item::Group(group), Item::Layer(alpha), Item::Layer(empty)] = doc.tree.as_slice() else {
        panic!("unexpected tree shape: {:?}", doc.tree);
    };
    assert_eq!(group.name, "Top group");
    assert!((group.opacity - 0.5).abs() < 1e-9);
    assert!(group.collapsed && !group.hidden);
    let [Item::Layer(ink), Item::Layer(hidden)] = group.children.as_slice() else {
        panic!("group should hold two layers");
    };
    assert_eq!(ink.name, "Ink");
    assert!(ink.locked);
    assert_eq!(ink.blend_name().as_deref(), Some("Multiply"));
    assert!(hidden.hidden && hidden.clipped);
    assert_eq!(hidden.blend_name().as_deref(), Some("Divide"));
    assert_eq!(alpha.name, "Transparency locked");
    assert!(alpha.alpha_locked && alpha.private);
    assert_eq!(alpha.blend_mode, Some(34));
    assert_eq!(empty.uuid, "EMPTY-0");

    let mut specs = Vec::new();
    flat_layers(&builder.tree, &mut specs);
    let layers = doc.layers.clone();
    assert_eq!(layers.len(), specs.len());
    for (layer, spec) in layers.iter().zip(&specs) {
        assert_eq!(layer.name, spec.name);
        assert_eq!(layer.hidden, spec.hidden);
        assert_eq!(layer.blend_mode, Some(spec.blend_mode));
        let want = spec
            .pixels
            .clone()
            .unwrap_or_else(|| vec![0; (W * H * 4) as usize]);
        assert!(doc.layer_rgba(layer).unwrap() == want, "pixels of layer {:?}", layer.name);
    }

    // 4 x 3 tiles; the 4 tiles in the transparent bottom-right corner are not stored
    assert_eq!(doc.tile_count(&layers[0].uuid), 8);
    assert_eq!(doc.tile_count(&layers[1].uuid), 8);
    assert_eq!(doc.tile_count(&layers[2].uuid), 8);
    assert_eq!(doc.tile_count(&layers[3].uuid), 0);
    assert_eq!(doc.composite_rgba().unwrap().map(|c| c.len()), Some((W * H * 4) as usize));
}

#[test]
fn other_tile_sizes_round_trip() {
    for tile in [100u32, 512] {
        let mut b = DocumentBuilder::new(W, H);
        b.tile_size = tile;
        b.tree = vec![Node::Layer(LayerSpec {
            name: "Only".into(),
            pixels: Some(pattern(9, W as usize, H as usize)),
            ..Default::default()
        })];
        let mut doc = reopen(&b);
        assert_eq!(doc.tile_size, tile);
        let layer = doc.layers[0].clone();
        assert!(doc.layer_rgba(&layer).unwrap() == pattern(9, W as usize, H as usize));
    }
}

#[test]
fn groups_hidden_layers_composite_and_thumbnail() {
    let mut b = DocumentBuilder::new(5, 3);
    b.tile_size = 2; // edge tiles are 1 pixel wide and 1 pixel high
    b.tree = vec![
        Node::Layer(LayerSpec {
            name: "Half".into(),
            pixels: Some(solid([0, 128, 0, 128], 5, 3)),
            ..Default::default()
        }),
        Node::Group(GroupSpec {
            name: "Hidden group".into(),
            hidden: true,
            children: vec![Node::Layer(LayerSpec {
                name: "Blue".into(),
                pixels: Some(solid([0, 0, 255, 255], 5, 3)),
                ..Default::default()
            })],
            ..Default::default()
        }),
        Node::Layer(LayerSpec {
            name: "Green".into(),
            hidden: true,
            pixels: Some(solid([0, 255, 0, 255], 5, 3)),
            ..Default::default()
        }),
        Node::Layer(LayerSpec {
            name: "Red".into(),
            pixels: Some(solid([255, 0, 0, 255], 5, 3)),
            ..Default::default()
        }),
    ];
    let bytes = b.to_bytes().unwrap();
    let mut doc = Document::from_reader(Cursor::new(bytes)).unwrap();

    let [Item::Layer(half), Item::Group(group), Item::Layer(green), Item::Layer(red)] =
        doc.tree.as_slice()
    else {
        panic!("unexpected tree shape");
    };
    assert_eq!(half.name, "Half");
    assert!(group.hidden);
    assert!(green.hidden && !red.hidden);
    assert_eq!(doc.layers.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), ["Half", "Blue", "Green", "Red"]);

    let layers = doc.layers.clone();
    assert!(doc.layer_rgba(&layers[0]).unwrap() == solid([0, 128, 0, 128], 5, 3));
    assert!(doc.layer_rgba(&layers[3]).unwrap() == solid([255, 0, 0, 255], 5, 3));

    // red, then the 50% green-tinted layer over it; hidden layers and groups are skipped
    let expected = [127u8, 128, 0, 255];
    let composite = doc.composite_rgba().unwrap().expect("composite is written");
    assert!(composite.chunks(4).all(|p| p == expected));

    let thumb = doc.thumbnail().unwrap().expect("thumbnail is written");
    assert_eq!(&thumb[..4], b"\x89PNG");
    let mut reader = png::Decoder::new(Cursor::new(thumb)).read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    assert_eq!((info.width, info.height), (5, 3));
    assert!(buf.chunks(4).all(|p| p == expected));
}

#[test]
fn thumbnail_is_downscaled_to_1024_on_the_long_side() {
    let (w, h) = (2048usize, 1000usize);
    let mut b = DocumentBuilder::new(w as u32, h as u32);
    b.tree = vec![Node::Layer(LayerSpec {
        name: "Gray".into(),
        pixels: Some(solid([90, 90, 90, 255], w, h)),
        ..Default::default()
    })];
    let mut doc = reopen(&b);
    let thumb = doc.thumbnail().unwrap().expect("thumbnail");
    let mut reader = png::Decoder::new(Cursor::new(thumb)).read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    assert_eq!((info.width, info.height), (1024, 500));
    assert!(buf.chunks(4).all(|p| p == [90, 90, 90, 255]));
}

#[test]
fn blend_modes_round_trip_and_legacy_fields() {
    let modes: Vec<i64> = (0..=17).chain(19..=26).chain([34]).collect();
    for mode in modes {
        let mut b = DocumentBuilder::new(4, 4);
        b.tree = vec![Node::Layer(LayerSpec {
            name: format!("mode {mode}"),
            blend_mode: mode,
            ..Default::default()
        })];
        let bytes = b.to_bytes().unwrap();
        let doc = Document::from_reader(Cursor::new(bytes.clone())).unwrap();
        assert_eq!(doc.layers[0].blend_mode, Some(mode));
        assert_eq!(doc.layers[0].blend_name(), blend_mode_name(mode).map(str::to_owned));

        // `blend` and `extendedBlend` are the legacy-safe copies
        let item = first_top_item(&bytes);
        let layer = item.as_archived().unwrap();
        let (blend, extended) = match mode {
            20..=33 => (0, mode),
            34 => (19, 19),
            _ => (mode, mode),
        };
        assert_eq!(layer.i64("blend"), Some(blend), "blend for {mode}");
        assert_eq!(layer.i64("extendedBlend"), Some(extended), "extendedBlend for {mode}");
        assert_eq!(layer.i64("extendedBlend2"), Some(mode));
    }

    for bad in [18, 27, 33, 35, -1] {
        let mut b = DocumentBuilder::new(4, 4);
        b.tree = vec![Node::Layer(LayerSpec {
            blend_mode: bad,
            ..Default::default()
        })];
        let err = b.to_bytes().unwrap_err();
        assert!(matches!(err, Error::InvalidInput(_)), "mode {bad}: {err}");
    }
}

#[test]
fn invalid_builders_are_rejected() {
    let mut b = DocumentBuilder::new(10, 10);
    b.tree = vec![Node::Layer(LayerSpec {
        pixels: Some(vec![0; 3]),
        ..Default::default()
    })];
    assert!(matches!(b.to_bytes().unwrap_err(), Error::InvalidInput(_)));

    let mut b = DocumentBuilder::new(10, 10);
    b.tile_size = 0;
    assert!(matches!(b.to_bytes().unwrap_err(), Error::InvalidInput(_)));

    let b = DocumentBuilder::new(0, 10);
    assert!(matches!(b.to_bytes().unwrap_err(), Error::InvalidInput(_)));

    let mut b = DocumentBuilder::new(10, 10);
    b.tree = vec![
        Node::Layer(LayerSpec { uuid: Some("SAME".into()), ..Default::default() }),
        Node::Layer(LayerSpec { uuid: Some("SAME".into()), ..Default::default() }),
    ];
    assert!(matches!(b.to_bytes().unwrap_err(), Error::InvalidInput(_)));

    let mut b = DocumentBuilder::new(10, 10);
    b.tree = vec![Node::Layer(LayerSpec { uuid: Some("a/b".into()), ..Default::default() })];
    assert!(matches!(b.to_bytes().unwrap_err(), Error::InvalidInput(_)));
}

#[test]
fn defaults_follow_the_builder_documentation() {
    let b = DocumentBuilder::new(7, 9);
    assert_eq!((b.width, b.height, b.tile_size), (7, 9, 256));
    assert_eq!(b.dpi, 132.0);
    assert_eq!(b.orientation, 1);
    assert_eq!(b.background_color, [1.0; 4]);
    let layer = LayerSpec::default();
    assert_eq!((layer.opacity, layer.blend_mode, layer.pixels.is_none()), (1.0, 0, true));
}

#[test]
fn write_is_atomic_and_readable_from_disk() {
    let dir = env::temp_dir().join(format!("eskiz-writer-test-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();

    let path = dir.join("sample.procreate");
    document::write(&path, &sample_builder()).unwrap();
    assert!(path.exists());
    assert!(!dir.join("sample.procreate.tmp").exists());
    let doc = Document::open(&path).unwrap();
    assert_eq!(doc.name.as_deref(), Some("Odd canvas"));
    // `Document` keeps the package file open. Under Win32 delete semantics a file that is still
    // open only disappears when its last handle closes, so removing the directory below would
    // fail with ERROR_DIR_NOT_EMPTY (Wine, FAT/exFAT volumes, Windows before 10 1607; NTFS on
    // current Windows gets POSIX-style unlinks from std). Close it before cleaning up.
    drop(doc);

    // a rejected builder leaves neither the target nor the temporary file behind
    let rejected = dir.join("rejected.procreate");
    let mut bad = DocumentBuilder::new(0, 0);
    bad.name = "bad".into();
    assert!(document::write(&rejected, &bad).is_err());
    assert!(!rejected.exists());
    assert!(!dir.join("rejected.procreate.tmp").exists());

    fs::remove_dir_all(&dir).unwrap();
}

/// Writes the sample document to the temp directory for an external cross-check with another reader.
/// The file is left in place on purpose.
#[test]
fn writes_crosscheck_sample_for_python_reader() {
    let path = env::temp_dir().join("eskiz-format-crosscheck.procreate");
    document::write(&path, &sample_builder()).unwrap();
    let doc = Document::open(&path).unwrap();
    let names: Vec<&str> = doc.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["Ink", "Hidden paint", "Transparency locked", "Empty"]);
}

#[test]
fn animation_defaults_follow_spec_4a() {
    let bytes = DocumentBuilder::new(4, 4).to_bytes().unwrap();
    let root = keyedarchive::unarchive(&archive_bytes(&bytes)).unwrap();
    let anim = root
        .as_archived()
        .unwrap()
        .archived("animation")
        .expect("animation object");
    assert_eq!(anim.class_name, "ValkyrieDocumentAnimation");
    assert_eq!(anim.f64("frameRate"), Some(15.0));
    assert_eq!(anim.i64("onionSkinCount"), Some(12));
    assert_eq!(anim.f64("onionSkinOpacity"), Some(0.6));
    assert!(anim.truthy("onionSkinEnabled"));
    assert!(!anim.truthy("primaryMixed"));
    assert_eq!(anim.i64("playbackMode"), Some(1));
    assert_eq!(anim.i64("skinBlendMode"), Some(2));
    assert_eq!(anim.i64("animationMode"), Some(0));
    assert_eq!(anim.i64("playbackDirection"), Some(0));
}
