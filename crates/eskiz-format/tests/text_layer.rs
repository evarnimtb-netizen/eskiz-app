//! A text layer's source survives a `.procreate` write and read (`text` key on `SilicaLayer`).

use std::fs;

use eskiz_format::{document, Document, DocumentBuilder, LayerSpec, Node, TextRunSpec, TextSpec, TextValue};

#[test]
fn text_layer_source_round_trips_through_procreate() {
    let (w, h) = (64u32, 48u32);
    let text = TextSpec {
        width: 120.0,
        text: "Hi\nÉ".to_owned(),
        runs: vec![
            TextRunSpec {
                len: 2,
                attributes: vec![
                    ("_CFN".to_owned(), TextValue::Text("Helvetica-Bold".to_owned())),
                    ("_CFZ".to_owned(), TextValue::Number(24.5)),
                    ("eskiz.underline".to_owned(), TextValue::Flag(true)),
                ],
            },
            TextRunSpec {
                len: 3,
                attributes: vec![("eskiz.colour".to_owned(), TextValue::Text("#1e3c8c".to_owned()))],
            },
        ],
        transform: [1.0, 0.0, 0.0, 1.0, 8.0, 9.0],
    };
    let mut pixels = vec![0u8; (w * h * 4) as usize];
    pixels[..4].copy_from_slice(&[255, 0, 0, 255]);

    let mut builder = DocumentBuilder::new(w, h);
    builder.tree = vec![
        Node::Layer(LayerSpec {
            name: "Eskiz".to_owned(),
            pixels: Some(pixels),
            text: Some(text.clone()),
            ..LayerSpec::default()
        }),
        Node::Layer(LayerSpec {
            name: "plain".to_owned(),
            ..LayerSpec::default()
        }),
    ];

    let dir = std::env::temp_dir().join(format!("eskiz-text-layer-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("text.procreate");
    document::write(&path, &builder).unwrap();

    let doc = Document::open(&path).unwrap();
    let _ = fs::remove_dir_all(&dir);

    // Layers are listed top-most first.
    let text_layer = &doc.layers[0];
    assert_eq!(text_layer.name, "Eskiz");
    assert!(text_layer.is_text);
    assert_eq!(text_layer.text.as_ref(), Some(&text));
    let plain = &doc.layers[1];
    assert!(!plain.is_text);
    assert!(plain.text.is_none());
}
