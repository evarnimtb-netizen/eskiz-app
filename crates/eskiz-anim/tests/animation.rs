//! Frames tests.

use eskiz_anim::{
    export_frames, onion_skins, render_with_onion, tint_premul, AnimationSettings, FrameList,
    OnionSide, Playback, PlaybackDirection, PlaybackMode, StackItem, StackSnapshot, Tick,
};
use eskiz_core::{BlendMode, Layer};

const W: usize = 2;
const H: usize = 2;

/// An opaque solid layer.
fn solid(rgb: [u8; 3]) -> Layer {
    Layer::from_rgba(W, H, [rgb[0], rgb[1], rgb[2], 255].repeat(W * H))
}

/// A fully transparent layer.
fn clear() -> Layer {
    Layer::new(W, H)
}

fn item(layer: Layer) -> StackItem {
    StackItem::new(layer)
}

fn snapshot(items: Vec<StackItem>) -> StackSnapshot {
    StackSnapshot {
        width: W,
        height: H,
        items,
    }
}

/// `n` frames, each a solid colour whose red channel is `10 * k`, so a frame is identified by red.
fn numbered_frames(n: u8) -> FrameList {
    let items = (0..n).map(|k| item(solid([k * 10, 0, 0]))).collect();
    FrameList::build(snapshot(items)).expect("valid stack")
}

fn settings_with(mode: PlaybackMode, direction: PlaybackDirection, fps: u32) -> AnimationSettings {
    AnimationSettings {
        playback_mode: mode,
        direction,
        frame_rate: fps,
        ..AnimationSettings::default()
    }
}

/// Plays from frame 0 at `fps` and records the frame shown at the start and after each tick.
fn samples(
    mode: PlaybackMode,
    direction: PlaybackDirection,
    frames: &FrameList,
    ticks: usize,
) -> Vec<usize> {
    let settings = settings_with(mode, direction, 3);
    let mut pb = Playback::new(&settings, 0);
    pb.play(frames);
    let mut out = vec![pb.frame()];
    for _ in 0..ticks {
        pb.advance(1.0 / 3.0, frames);
        out.push(pb.frame());
    }
    out
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

#[test]
fn defaults_match_the_spec() {
    let s = AnimationSettings::default();
    assert!(!s.frames_enabled);
    assert_eq!(s.frame_rate, 15);
    assert_eq!(s.playback_mode, PlaybackMode::Loop);
    assert_eq!(s.direction, PlaybackDirection::Forward);
    assert!(s.onion_enabled);
    assert_eq!(s.onion_count, 12);
    assert!((s.onion_opacity - 0.6).abs() < 1e-12);
    assert_eq!(s.previous_colour, [0xE5, 0x48, 0x4D]);
    assert_eq!(s.next_colour, [0x3B, 0x82, 0xF6]);
    assert!(!s.primary_mixed);
    assert_eq!(s.skin_blend_mode, 2);
    assert_eq!(s.onion_blend_mode(), BlendMode::Normal);
    assert_eq!(s.export_frame_rate, None);
    assert_eq!(s.export_fps(), 15);
}

#[test]
fn frame_rate_is_clamped_to_one_to_sixty() {
    let mut s = AnimationSettings::default();
    s.set_frame_rate(0);
    assert_eq!(s.frame_rate, 1);
    s.set_frame_rate(100);
    assert_eq!(s.frame_rate, 60);
    s.frame_rate = 100;
    assert_eq!(s.fps(), 60);
    s.frame_rate = 0;
    assert_eq!(s.fps(), 1);
    s.export_frame_rate = Some(0);
    assert_eq!(s.export_fps(), 1);
    s.export_frame_rate = Some(500);
    assert_eq!(s.export_fps(), 60);
}

#[test]
fn onion_count_and_opacity_are_clamped() {
    let low = AnimationSettings {
        onion_count: 0,
        ..AnimationSettings::default()
    };
    assert_eq!(low.onion_count(), 1);
    let high = AnimationSettings {
        onion_count: 99,
        ..AnimationSettings::default()
    };
    assert_eq!(high.onion_count(), 20);
    let over = AnimationSettings {
        onion_opacity: 1.7,
        ..AnimationSettings::default()
    };
    assert!((over.onion_opacity() - 1.0).abs() < 1e-12);
    let under = AnimationSettings {
        onion_opacity: -0.2,
        ..AnimationSettings::default()
    };
    assert!(under.onion_opacity().abs() < 1e-12);
}

#[test]
fn multiply_onion_blend_applies_only_with_primary_mixed() {
    let mut s = AnimationSettings {
        onion_blend: eskiz_anim::OnionBlend::Multiply,
        ..AnimationSettings::default()
    };
    assert_eq!(s.onion_blend_mode(), BlendMode::Normal);
    s.primary_mixed = true;
    assert_eq!(s.onion_blend_mode(), BlendMode::Multiply);
}

// ---------------------------------------------------------------------------
// Frame list
// ---------------------------------------------------------------------------

#[test]
fn stack_rules_give_the_predicted_frame_count() {
    // Bottom to top: background, A, clipped D (joins A), hidden B, private C, E, foreground F.
    let mut background = item(solid([1, 1, 1]));
    background.animation_background = true;
    let mut a = item(solid([10, 0, 0]));
    a.hold = 1;
    let mut hidden = item(solid([20, 0, 0]));
    hidden.layer.visible = false;
    let mut private = item(solid([30, 0, 0]));
    private.private = true;
    let mut clipped = item(solid([40, 0, 0]));
    clipped.clipped = true;
    let mut e = item(solid([50, 0, 0]));
    e.hold = 3;
    let mut foreground = item(solid([60, 0, 0]));
    foreground.animation_foreground = true;

    let list = FrameList::build(snapshot(vec![
        background, a, clipped, hidden, private, e, foreground,
    ]))
    .expect("valid stack");

    assert_eq!(list.len(), 2);
    assert_eq!(list.frame(0).map(|f| f.items.clone()), Some(vec![1, 2]));
    assert_eq!(list.frame(1).map(|f| f.items.clone()), Some(vec![5]));
    assert_eq!(
        list.hold(0),
        1,
        "hold comes from the bottom item of the frame"
    );
    assert_eq!(list.hold(1), 3);
    assert_eq!(list.background_item(), Some(0));
    assert_eq!(list.foreground_item(), Some(6));
}

#[test]
fn clipped_item_with_nothing_below_starts_its_own_frame() {
    let mut clipped = item(solid([9, 0, 0]));
    clipped.clipped = true;
    let list = FrameList::build(snapshot(vec![clipped])).expect("valid stack");
    assert_eq!(list.len(), 1);
    assert_eq!(list.frame(0).map(|f| f.items.clone()), Some(vec![0]));
}

#[test]
fn clipped_item_over_a_hidden_item_starts_its_own_frame() {
    let mut hidden = item(solid([9, 0, 0]));
    hidden.layer.visible = false;
    let mut clipped = item(solid([8, 0, 0]));
    clipped.clipped = true;
    let list = FrameList::build(snapshot(vec![hidden, clipped])).expect("valid stack");
    assert_eq!(list.len(), 1);
    assert_eq!(list.frame(0).map(|f| f.items.clone()), Some(vec![1]));
}

#[test]
fn hold_is_clamped_to_thirty() {
    let mut a = item(solid([1, 0, 0]));
    a.hold = 99;
    let list = FrameList::build(snapshot(vec![a])).expect("valid stack");
    assert_eq!(list.hold(0), 30);
}

#[test]
fn mismatched_item_size_is_rejected() {
    let bad = StackItem::new(Layer::new(3, 3));
    let err = FrameList::build(snapshot(vec![bad])).unwrap_err();
    assert_eq!(
        err,
        eskiz_anim::AnimError::ItemSize {
            index: 0,
            width: W,
            height: H
        }
    );
}

// ---------------------------------------------------------------------------
// Playback
// ---------------------------------------------------------------------------

#[test]
fn loop_shows_zero_one_two_zero_over_one_second() {
    let frames = numbered_frames(3);
    assert_eq!(
        samples(PlaybackMode::Loop, PlaybackDirection::Forward, &frames, 3),
        vec![0, 1, 2, 0]
    );
}

#[test]
fn ping_pong_shows_the_turnaround_twice() {
    let frames = numbered_frames(3);
    assert_eq!(
        samples(
            PlaybackMode::PingPong,
            PlaybackDirection::Forward,
            &frames,
            8
        ),
        vec![0, 1, 2, 2, 1, 0, 0, 1, 2]
    );
}

#[test]
fn one_shot_stops_on_the_last_frame() {
    let frames = numbered_frames(3);
    let settings = settings_with(PlaybackMode::OneShot, PlaybackDirection::Forward, 3);
    let mut pb = Playback::new(&settings, 0);
    pb.play(&frames);
    let mut seen = vec![pb.frame()];
    for _ in 0..6 {
        pb.advance(1.0 / 3.0, &frames);
        seen.push(pb.frame());
    }
    assert_eq!(seen, vec![0, 1, 2, 2, 2, 2, 2]);
    assert!(!pb.is_playing());
    assert!(pb.is_finished());
    assert_eq!(
        pb.advance(1.0, &frames),
        0,
        "a finished one-shot does not wrap"
    );
}

#[test]
fn one_shot_reverse_stops_on_the_first_frame() {
    let frames = numbered_frames(3);
    let settings = settings_with(PlaybackMode::OneShot, PlaybackDirection::Reverse, 3);
    let mut pb = Playback::new(&settings, 2);
    pb.play(&frames);
    assert_eq!(pb.frame(), 2);
    let mut seen = vec![pb.frame()];
    for _ in 0..4 {
        pb.advance(1.0 / 3.0, &frames);
        seen.push(pb.frame());
    }
    assert_eq!(seen, vec![2, 1, 0, 0, 0]);
    assert!(pb.is_finished());
}

#[test]
fn reverse_loop_wraps_from_first_to_last() {
    let frames = numbered_frames(3);
    assert_eq!(
        samples(PlaybackMode::Loop, PlaybackDirection::Reverse, &frames, 3),
        vec![0, 2, 1, 0]
    );
}

#[test]
fn held_frame_is_shown_for_hold_plus_one_ticks() {
    // Frame 0 has hold 2, so it shows for three ticks before the playhead moves on.
    let mut held = item(solid([0, 0, 0]));
    held.hold = 2;
    let list = FrameList::build(snapshot(vec![held, item(solid([10, 0, 0]))])).expect("valid");
    assert_eq!(
        samples(PlaybackMode::Loop, PlaybackDirection::Forward, &list, 3),
        vec![0, 0, 0, 1]
    );
}

#[test]
fn step_moves_one_tick_and_pauses() {
    let frames = numbered_frames(3);
    let settings = settings_with(PlaybackMode::Loop, PlaybackDirection::Forward, 3);
    let mut pb = Playback::new(&settings, 0);
    pb.play(&frames);
    pb.pause();
    assert_eq!(pb.step(&frames), Tick::Moved);
    assert_eq!(pb.frame(), 1);
    assert!(!pb.is_playing());
    assert_eq!(pb.advance(1.0, &frames), 0, "paused playback does not move");
}

#[test]
fn clock_carries_the_leftover_fraction() {
    let frames = numbered_frames(3);
    let settings = settings_with(PlaybackMode::Loop, PlaybackDirection::Forward, 3);
    let mut pb = Playback::new(&settings, 0);
    pb.play(&frames);
    // Ten 0.1 s updates at 3 fps make exactly three ticks in total.
    let total: u32 = (0..10).map(|_| pb.advance(0.1, &frames)).sum();
    assert_eq!(total, 3);
    assert_eq!(
        pb.frame(),
        0,
        "three ticks around a three-frame loop return to frame 0"
    );
}

#[test]
fn half_a_tick_runs_nothing_until_the_rest_arrives() {
    let frames = numbered_frames(3);
    let settings = settings_with(PlaybackMode::Loop, PlaybackDirection::Forward, 3);
    let mut pb = Playback::new(&settings, 0);
    pb.play(&frames);
    assert_eq!(pb.advance(0.5 / 3.0, &frames), 0);
    assert_eq!(pb.advance(0.5 / 3.0, &frames), 1);
    assert_eq!(pb.frame(), 1);
}

#[test]
fn scrub_sets_the_frame_and_pauses() {
    let frames = numbered_frames(3);
    let settings = settings_with(PlaybackMode::Loop, PlaybackDirection::Forward, 3);
    let mut pb = Playback::new(&settings, 0);
    pb.play(&frames);
    pb.advance(1.0 / 3.0, &frames);
    pb.scrub(2, &frames);
    assert_eq!(pb.frame(), 2);
    assert!(!pb.is_playing());
    assert_eq!(pb.advance(1.0, &frames), 0);
    assert_eq!(pb.frame(), 2);
    pb.scrub(99, &frames);
    assert_eq!(
        pb.frame(),
        2,
        "an index past the end selects the last frame"
    );
}

#[test]
fn playing_after_a_one_shot_restarts_from_the_first_frame() {
    let frames = numbered_frames(2);
    let settings = settings_with(PlaybackMode::OneShot, PlaybackDirection::Forward, 2);
    let mut pb = Playback::new(&settings, 0);
    pb.play(&frames);
    pb.advance(10.0, &frames);
    assert!(pb.is_finished());
    pb.play(&frames);
    assert_eq!(pb.frame(), 0);
    assert!(pb.is_playing());
}

// ---------------------------------------------------------------------------
// Onion skins
// ---------------------------------------------------------------------------

#[test]
fn onion_selection_counts_at_the_edges() {
    let frames = numbered_frames(5);
    let settings = AnimationSettings {
        onion_count: 2,
        ..AnimationSettings::default()
    };
    let before = |c: usize| {
        onion_skins(&settings, &frames, c, false)
            .iter()
            .filter(|s| s.side == OnionSide::Before)
            .count()
    };
    let after = |c: usize| {
        onion_skins(&settings, &frames, c, false)
            .iter()
            .filter(|s| s.side == OnionSide::After)
            .count()
    };
    // First frame: nothing before, two after.
    assert_eq!((before(0), after(0)), (0, 2));
    // Middle frame: two on each side.
    assert_eq!((before(2), after(2)), (2, 2));
    // Last frame: two before, nothing after.
    assert_eq!((before(4), after(4)), (2, 0));
    // A count larger than the stack only shows the frames that exist.
    let wide = AnimationSettings {
        onion_count: 12,
        ..AnimationSettings::default()
    };
    assert_eq!(onion_skins(&wide, &frames, 1, false).len(), 1 + 3);
}

#[test]
fn onion_opacity_falls_off_by_three_quarters_per_frame() {
    let frames = numbered_frames(5);
    let settings = AnimationSettings::default();
    let skins = onion_skins(&settings, &frames, 2, false);
    let before: Vec<_> = skins
        .iter()
        .filter(|s| s.side == OnionSide::Before)
        .collect();
    assert_eq!(before[0].frame, 1);
    assert_eq!(before[0].distance, 1);
    assert!((before[0].opacity - 0.6).abs() < 1e-12);
    assert!((before[1].opacity - 0.45).abs() < 1e-12);
    assert!(before.iter().all(|s| s.tint == [0xE5, 0x48, 0x4D]));
    let after: Vec<_> = skins
        .iter()
        .filter(|s| s.side == OnionSide::After)
        .collect();
    assert_eq!(after[0].frame, 3);
    assert!(after.iter().all(|s| s.tint == [0x3B, 0x82, 0xF6]));
}

#[test]
fn no_onion_skins_while_playing_or_when_disabled_or_empty() {
    let frames = numbered_frames(3);
    let settings = AnimationSettings::default();
    assert!(onion_skins(&settings, &frames, 1, true).is_empty());
    let off = AnimationSettings {
        onion_enabled: false,
        ..AnimationSettings::default()
    };
    assert!(onion_skins(&off, &frames, 1, false).is_empty());
    let empty = FrameList::build(snapshot(Vec::new())).expect("empty stack is valid");
    assert!(onion_skins(&settings, &empty, 0, false).is_empty());
}

#[test]
fn tint_keeps_the_shape_and_takes_the_tint_colour() {
    let t = tint_premul(0.5, [255, 0, 0]);
    assert_eq!(t, [0.5, 0.0, 0.0, 0.5]);
    let blue = tint_premul(1.0, [0, 0, 255]);
    assert_eq!(blue, [0.0, 0.0, 1.0, 1.0]);
    assert_eq!(tint_premul(0.0, [9, 9, 9]), [0.0; 4]);
    assert_eq!(
        tint_premul(3.0, [0, 0, 0]),
        [0.0, 0.0, 0.0, 1.0],
        "alpha clamps to 1"
    );
}

#[test]
fn onion_frame_is_drawn_tinted_beneath_the_current_frame() {
    // Frame 0 is opaque black. Frame 1 is transparent, so the onion skin of frame 0 shows through.
    let frames = FrameList::build(snapshot(vec![item(solid([0, 0, 0])), item(clear())]))
        .expect("valid stack");
    let settings = AnimationSettings {
        onion_opacity: 0.5,
        ..AnimationSettings::default()
    };
    let skins = onion_skins(&settings, &frames, 1, false);
    assert_eq!(skins.len(), 1);
    let blue = eskiz_anim::OnionSkin {
        tint: [0, 0, 255],
        ..skins[0]
    };
    let out = render_with_onion(&frames, 1, &[blue], BlendMode::Normal);
    // Half blue over white paper: red and green near 128, blue full.
    assert!((i32::from(out[0]) - 128).abs() <= 2, "red {}", out[0]);
    assert!((i32::from(out[1]) - 128).abs() <= 2, "green {}", out[1]);
    assert_eq!(out[2], 255);
    assert_eq!(out[3], 255);
}

#[test]
fn current_frame_is_never_tinted_and_covers_the_onion_skin() {
    let frames = FrameList::build(snapshot(vec![
        item(solid([0, 0, 0])),
        item(solid([0, 255, 0])),
    ]))
    .expect("valid stack");
    let skins = onion_skins(&AnimationSettings::default(), &frames, 1, false);
    let out = render_with_onion(&frames, 1, &skins, BlendMode::Normal);
    assert_eq!(&out[0..4], &[0, 255, 0, 255]);
}

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------

#[test]
fn export_repeats_holds_at_one_over_fps() {
    // Holds 0, 2 and 0: three pictures lasting five ticks in total at 15 fps.
    let mut a = item(solid([10, 0, 0]));
    a.hold = 0;
    let mut b = item(solid([20, 0, 0]));
    b.hold = 2;
    let mut c = item(solid([30, 0, 0]));
    c.hold = 0;
    let frames = FrameList::build(snapshot(vec![a, b, c])).expect("valid stack");
    let out = export_frames(&AnimationSettings::default(), &frames);
    assert_eq!(out.len(), 3);
    let reds: Vec<u8> = out.iter().map(|f| f.pixels()[0]).collect();
    assert_eq!(reds, vec![10, 20, 30]);
    let one_frame = eskiz_media::duration_for_fps(15);
    let ticks: Vec<u32> = vec![1, 3, 1];
    for (f, t) in out.iter().zip(ticks) {
        assert_eq!(f.duration(), one_frame * t);
        assert_eq!((f.width(), f.height()), (W as u32, H as u32));
    }
    let total: f64 = out.iter().map(|f| f.duration().as_secs_f64()).sum();
    assert!((total - 5.0 / 15.0).abs() < 1e-6, "total {total}");
}

#[test]
fn export_honours_the_export_frame_rate() {
    let frames = numbered_frames(3);
    let settings = AnimationSettings {
        export_frame_rate: Some(10),
        ..AnimationSettings::default()
    };
    let out = export_frames(&settings, &frames);
    assert_eq!(out.len(), 3);
    let total: f64 = out.iter().map(|f| f.duration().as_secs_f64()).sum();
    assert!((total - 0.3).abs() < 1e-6);
}

#[test]
fn reverse_direction_exports_in_reverse_order() {
    let mut b = item(solid([20, 0, 0]));
    b.hold = 2;
    let frames = FrameList::build(snapshot(vec![
        item(solid([10, 0, 0])),
        b,
        item(solid([30, 0, 0])),
    ]))
    .expect("valid stack");
    let settings = AnimationSettings {
        direction: PlaybackDirection::Reverse,
        ..AnimationSettings::default()
    };
    let out = export_frames(&settings, &frames);
    let reds: Vec<u8> = out.iter().map(|f| f.pixels()[0]).collect();
    assert_eq!(reds, vec![30, 20, 10]);
    assert_eq!(out[1].duration(), eskiz_media::duration_for_fps(15) * 3);
}

#[test]
fn export_includes_the_background_and_excludes_hidden_and_private() {
    let mut background = item(solid([0, 0, 255]));
    background.animation_background = true;
    let mut hidden = item(solid([255, 0, 0]));
    hidden.layer.visible = false;
    let mut private = item(solid([255, 0, 0]));
    private.private = true;
    // The only frame is transparent, so the background shows through.
    let frames = FrameList::build(snapshot(vec![background, item(clear()), hidden, private]))
        .expect("valid stack");
    let out = export_frames(&AnimationSettings::default(), &frames);
    assert_eq!(out.len(), 1);
    assert_eq!(&out[0].pixels()[0..4], &[0, 0, 255, 255]);
}

#[test]
fn export_of_an_empty_stack_is_empty() {
    let frames = FrameList::build(snapshot(Vec::new())).expect("empty stack is valid");
    assert!(export_frames(&AnimationSettings::default(), &frames).is_empty());
}

#[test]
fn clipped_layer_does_not_paint_outside_its_base() {
    let mut px = vec![0u8; W * H * 4];
    px[..4].copy_from_slice(&[255, 0, 0, 255]);
    let base = item(Layer::from_rgba(W, H, px));
    let mut shade = item(solid([0, 0, 255]));
    shade.clipped = true;
    let frames = FrameList::build(snapshot(vec![base, shade])).expect("valid stack");
    assert_eq!(frames.len(), 1);
    let out = frames.render(0);
    assert_eq!(&out[..4], &[0, 0, 255, 255], "inside the base");
    assert_eq!(
        &out[4..8],
        &[255, 255, 255, 255],
        "outside the base stays paper"
    );
    let t = frames.render_transparent(0);
    assert_eq!(t[7], 0, "no spill in the transparent export");
}
