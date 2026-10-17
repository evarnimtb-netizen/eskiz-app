//! Guided Layer: grid snapping, isometric and perspective locking, and symmetry copies.

use eskiz_guides::{
    DrawingAssist, Grid, Isometric, Perspective, Sample, SnapGuide, Symmetry, SymmetryKind, Vec2,
};

const TOL: f64 = 1e-9;

fn close_pt(a: Vec2, b: Vec2) -> bool {
    (a.x - b.x).abs() < TOL && (a.y - b.y).abs() < TOL
}

fn s(x: f64, y: f64) -> Sample {
    Sample::new(Vec2::new(x, y), 0.5)
}

#[test]
fn grid_snap_picks_nearest_intersection() {
    let grid = Grid::new(10.0);
    assert!(close_pt(
        grid.snap(Vec2::new(14.0, 26.0)),
        Vec2::new(10.0, 30.0)
    ));
    assert!(close_pt(
        grid.snap(Vec2::new(-6.0, -4.0)),
        Vec2::new(-10.0, 0.0)
    ));
    // Exactly on an intersection stays put.
    assert!(close_pt(
        grid.snap(Vec2::new(20.0, -30.0)),
        Vec2::new(20.0, -30.0)
    ));
}

#[test]
fn rotated_grid_snap_recovers_lattice_point() {
    let mut grid = Grid::new(20.0);
    grid.rotation = 0.6;
    grid.origin = Vec2::new(5.0, 5.0);
    let (u, v) = grid.axes();
    let lattice = grid.origin + u * 60.0 + v * (-40.0);
    // Noise of 3 px along u stays under half a cell, so it snaps back.
    let noisy = lattice + u * 3.0;
    assert!(close_pt(grid.snap(noisy), lattice));
}

#[test]
fn grid_assist_snaps_every_sample_and_keeps_pressure() {
    let assist = DrawingAssist {
        snap: Some(SnapGuide::Grid(Grid::new(10.0))),
        ..DrawingAssist::default()
    };
    let out = assist.assist(&[
        Sample::new(Vec2::new(14.0, 26.0), 0.3),
        Sample::new(Vec2::new(41.0, 2.0), 0.7),
    ]);
    assert_eq!(out.len(), 1);
    assert!(close_pt(out[0][0].pos, Vec2::new(10.0, 30.0)));
    assert!(close_pt(out[0][1].pos, Vec2::new(40.0, 0.0)));
    assert_eq!(out[0][1].pressure, 0.7);
}

#[test]
fn isometric_locks_to_the_nearest_axis_after_the_threshold() {
    let assist = DrawingAssist {
        snap: Some(SnapGuide::Isometric(Isometric::new(10.0))),
        lock_distance: 16.0,
        ..DrawingAssist::default()
    };
    let samples = [
        s(0.0, 0.0),
        s(5.0, 0.0),
        s(10.0, 6.0),
        s(30.0, 10.0),
        s(60.0, 0.0),
    ];
    let out = assist.assist(&samples);
    let stroke = &out[0];
    // Before the lock distance, samples pass through unchanged.
    assert!(close_pt(stroke[0].pos, Vec2::new(0.0, 0.0)));
    assert!(close_pt(stroke[1].pos, Vec2::new(5.0, 0.0)));
    assert!(close_pt(stroke[2].pos, Vec2::new(10.0, 6.0)));
    // After it, the stroke lies on the 30-degree axis through the anchor.
    let axis = Vec2::from_angle(30f64.to_radians());
    for sample in &stroke[3..] {
        assert!(
            sample.pos.cross(axis).abs() < 1e-9,
            "off axis: {:?}",
            sample.pos
        );
    }
    // The projection of (30, 10) onto the axis is (30*cos30 + 10*sin30) * axis.
    let expected = axis * (30.0 * axis.x + 10.0 * axis.y);
    assert!(close_pt(stroke[3].pos, expected));
}

#[test]
fn isometric_axis_choice_is_sign_agnostic() {
    let iso = Isometric::new(10.0);
    // A drag going up-left is still nearest the 30-degree axis.
    let axis = iso.nearest_axis(Vec2::new(-1.0, -0.6));
    assert!(axis.cross(Vec2::from_angle(30f64.to_radians())).abs() < 1e-9);
}

#[test]
fn perspective_picks_the_vanishing_point_from_the_drag_and_locks() {
    let vp_horizontal = Vec2::new(1000.0, 400.0);
    let vp_vertical = Vec2::new(200.0, -2000.0);
    let assist = DrawingAssist {
        snap: Some(SnapGuide::Perspective(Perspective::two_point(
            vp_horizontal,
            vp_vertical,
        ))),
        lock_distance: 16.0,
        ..DrawingAssist::default()
    };
    let samples = [
        s(200.0, 400.0),
        s(300.0, 420.0),
        s(400.0, 480.0),
        // A later drag points at the vertical vanishing point, but the lock holds.
        s(210.0, 900.0),
    ];
    let out = assist.assist(&samples);
    let stroke = &out[0];
    assert!(close_pt(stroke[0].pos, Vec2::new(200.0, 400.0)));
    // Locked to the horizontal line through the anchor: every later sample has y = 400.
    assert!(close_pt(stroke[1].pos, Vec2::new(300.0, 400.0)));
    assert!(close_pt(stroke[2].pos, Vec2::new(400.0, 400.0)));
    assert!(close_pt(stroke[3].pos, Vec2::new(210.0, 400.0)));
}

#[test]
fn perspective_picks_the_vertical_point_for_a_vertical_drag() {
    let assist = DrawingAssist {
        snap: Some(SnapGuide::Perspective(Perspective::two_point(
            Vec2::new(1000.0, 400.0),
            Vec2::new(200.0, -2000.0),
        ))),
        ..DrawingAssist::default()
    };
    let out = assist.assist(&[s(200.0, 400.0), s(203.0, 300.0), s(204.0, 200.0)]);
    // Locked to the vertical line x = 200 (drag direction is near (0, -1)).
    assert!((out[0][2].pos.x - 200.0).abs() < 1e-6);
}

#[test]
fn perspective_stays_raw_when_no_vanishing_point_is_usable() {
    // The only vanishing point sits on the anchor, so no line direction exists.
    let assist = DrawingAssist {
        snap: Some(SnapGuide::Perspective(Perspective::one_point(Vec2::new(
            0.0, 0.0,
        )))),
        ..DrawingAssist::default()
    };
    let samples = [s(0.0, 0.0), s(50.0, 7.0), s(90.0, 3.0)];
    let out = assist.assist(&samples);
    for (got, want) in out[0].iter().zip(samples) {
        assert!(close_pt(got.pos, want.pos));
    }
}

#[test]
fn streaming_matches_whole_stroke_assist() {
    let assist = DrawingAssist {
        snap: Some(SnapGuide::Isometric(Isometric::new(8.0))),
        ..DrawingAssist::default()
    };
    let samples = [s(1.0, 1.0), s(30.0, 12.0), s(70.0, 40.0), s(90.0, 5.0)];
    let whole = assist.assist(&samples);
    let mut stream = assist.begin_stroke();
    let mut pushed: Vec<Vec<Sample>> = vec![Vec::new()];
    for sample in samples {
        for (dst, out) in pushed.iter_mut().zip(stream.push(sample)) {
            dst.push(out);
        }
    }
    assert_eq!(whole, pushed);
}

#[test]
fn radial_symmetry_turns_each_stroke_into_n_strokes() {
    let centre = Vec2::new(100.0, 100.0);
    let assist = DrawingAssist {
        symmetry: Some(Symmetry::new(
            SymmetryKind::Radial {
                segments: 5,
                mirrored: false,
            },
            centre,
        )),
        ..DrawingAssist::default()
    };
    assert_eq!(assist.output_count(), 5);
    let out = assist.assist(&[s(120.0, 100.0), s(120.0, 130.0), s(90.0, 90.0)]);
    assert_eq!(out.len(), 5);
    for stroke in &out {
        assert_eq!(stroke.len(), 3);
    }
    // Stroke 0 is the input itself.
    assert!(close_pt(out[0][0].pos, Vec2::new(120.0, 100.0)));
    // Stroke 1 is stroke 0 turned by 2*pi/5 about the centre.
    let turned = Vec2::new(20.0, 0.0).rotate(2.0 * std::f64::consts::PI / 5.0) + centre;
    assert!(close_pt(out[1][0].pos, turned));
}

#[test]
fn mirrored_symmetry_applies_after_snapping() {
    let assist = DrawingAssist {
        snap: Some(SnapGuide::Grid(Grid::new(10.0))),
        symmetry: Some(Symmetry::new(SymmetryKind::Vertical, Vec2::new(50.0, 0.0))),
        ..DrawingAssist::default()
    };
    let out = assist.assist(&[s(23.0, 4.0)]);
    // Snapped to (20, 0), then mirrored across x = 50 to (80, 0).
    assert!(close_pt(out[0][0].pos, Vec2::new(20.0, 0.0)));
    assert!(close_pt(out[1][0].pos, Vec2::new(80.0, 0.0)));
}

#[test]
fn no_guide_and_no_symmetry_is_identity() {
    let samples = [s(1.5, 2.5), s(9.0, -3.0)];
    let out = DrawingAssist::default().assist(&samples);
    assert_eq!(out, vec![samples.to_vec()]);
}
