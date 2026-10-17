//! Text attributes saved by releases that used the old app name (`easel.*` keys) still load, // legacy-easel
//! and new saves use `eskiz.*`.

use eskiz_text::storage::{apply_saved, attrs_to_saved, SavedValue, LEGACY_PREFIX, PREFIX};
use eskiz_text::Attrs;

#[test]
fn old_prefix_keys_load() {
    let old: &[(&str, SavedValue)] = &[ // legacy-easel
        ("easel.colour", SavedValue::Text("#1e3c8cff".into())), // legacy-easel
        ("easel.underline", SavedValue::Flag(true)), // legacy-easel
        ("easel.baselineShift", SavedValue::Number(2.5)), // legacy-easel
        ("easel.alignment", SavedValue::Text("right".into())), // legacy-easel
        ("easel.leading", SavedValue::Number(1.5)), // legacy-easel
        ("easel.vertical", SavedValue::Flag(true)), // legacy-easel
        ("easel.allCaps", SavedValue::Flag(true)), // legacy-easel
        ("easel.writingDirection", SavedValue::Text("ltr".into())), // legacy-easel
    ];
    let mut attrs = Attrs::default();
    for (k, v) in old {
        assert_eq!(apply_saved(&mut attrs, k, v), Ok(true), "{k}");
    }
    assert!(attrs.underline && attrs.vertical && attrs.all_caps);
    assert_eq!(attrs.color.to_hex(), "#1e3c8cff");
    assert_eq!(attrs.baseline_shift_pt, 2.5);
}

#[test]
fn new_saves_use_the_new_prefix_and_round_trip() {
    let attrs = Attrs { underline: true, all_caps: true, ..Attrs::default() };
    let saved = attrs_to_saved(&attrs);
    assert!(saved.iter().all(|(k, _)| !k.starts_with(LEGACY_PREFIX)));
    assert!(saved.iter().any(|(k, _)| k.starts_with(PREFIX)));
    let mut back = Attrs::default();
    for (k, v) in &saved {
        assert_eq!(apply_saved(&mut back, k, v), Ok(true));
    }
    assert!(back.underline && back.all_caps);
}
