//! Guide line generation, grid and isometric geometry, perspective and symmetry.

use eskiz_guides::{
    Grid, GuideRender, Isometric, Perspective, PerspectiveKind, Rect, Sample, Segment, Symmetry,
    SymmetryKind, Vec2,
};

const TOL: f64 = 1e-9;

fn viewport() -> Rect {
    Rect::new(0.0, 0.0, 500.0, 300.0)
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < TOL
}

fn close_pt(a: Vec2, b: Vec2) -> bool {
    close(a.x, b.x) && close(a.y, b.y)
}

fn inside(seg: &Segment, r: Rect) -> bool {
    let grown = Rect {
        min: r.min - Vec2::new(TOL, TOL),
        max: r.max + Vec2::new(TOL, TOL),
    };
    grown.contains(seg.a) && grown.contains(seg.b)
}

/// Direction of a segment in degrees, folded into `[0, 180)`.
fn direction_deg(seg: &Segment) -> f64 {
    let d = seg.b - seg.a;
    d.y.atan2(d.x).to_degrees().rem_euclid(180.0)
}

/// Whether `p` lies on the infinite line through `seg`.
fn on_line(seg: &Segment, p: Vec2) -> bool {
    let d = seg.b - seg.a;
    (d.cross(p - seg.a) / d.length()).abs() < 1e-6
}

#[test]
fn grid_line_count_matches_spacing() {
    // 500 x 300 viewport, spacing 100: verticals at x = 0..=500 (6), horizontals at
    // y = 0..=300 (4). Lines on the edges are kept.
    let lines = Grid::new(100.0).lines(viewport());
    assert_eq!(lines.len(), 10);
    assert!(lines.iter().all(|s| inside(s, viewport())));
}

#[test]
fn rotated_grid_lines_are_clipped_and_nonempty() {
    let mut grid = Grid::new(40.0);
    grid.rotation = 0.6;
    grid.origin = Vec2::new(13.0, -7.0);
    let lines = grid.lines(viewport());
    assert!(!lines.is_empty());
    assert!(lines.iter().all(|s| inside(s, viewport())));
}

#[test]
fn invalid_spacing_produces_no_lines() {
    assert!(Grid::new(0.0).lines(viewport()).is_empty());
    assert!(Grid::new(-5.0).lines(viewport()).is_empty());
    assert!(Grid::new(f64::NAN).lines(viewport()).is_empty());
}

#[test]
fn grid_render_applies_style() {
    let mut grid = Grid::new(100.0);
    grid.style = eskiz_guides::GuideStyle::new(0.25, 2.0);
    let rendered = grid.render(viewport());
    assert_eq!(rendered.len(), 10);
    assert!(rendered
        .iter()
        .all(|s| close(s.opacity, 0.25) && close(s.thickness, 2.0)));
}

#[test]
fn isometric_lines_use_only_the_three_axes() {
    let lines = Isometric::new(50.0).lines(viewport());
    assert!(!lines.is_empty());
    for seg in &lines {
        let dir = direction_deg(seg);
        let ok = [30.0, 90.0, 150.0].iter().any(|a| (dir - a).abs() < 1e-6);
        assert!(ok, "unexpected direction {dir}");
        assert!(inside(seg, viewport()));
    }
}

#[test]
fn one_point_perspective_fan_has_one_line_per_direction() {
    let vp = Vec2::new(250.0, 150.0);
    let mut p = Perspective::one_point(vp);
    p.show_horizon = false;
    let lines = p.lines(viewport());
    assert_eq!(lines.len(), 12);
    assert!(lines.iter().all(|s| on_line(s, vp)));
}

#[test]
fn two_point_perspective_with_horizon_adds_one_line() {
    let a = Vec2::new(150.0, 150.0);
    let b = Vec2::new(350.0, 120.0);
    let p = Perspective::two_point(a, b);
    let lines = p.lines(viewport());
    // Interior vanishing points: each fan line crosses the viewport. Plus the horizon.
    assert_eq!(lines.len(), 12 + 12 + 1);
    let (point, dir) = p.horizon();
    assert!(close_pt(point, a));
    assert!(dir.dot(b - a).abs() > 0.0);
    assert!(on_line(&lines[lines.len() - 1], a));
    assert!(on_line(&lines[lines.len() - 1], b));
}

#[test]
fn three_point_perspective_counts_all_fans_and_horizon() {
    let p = Perspective::three_point(
        Vec2::new(100.0, 150.0),
        Vec2::new(400.0, 150.0),
        Vec2::new(250.0, 200.0),
    );
    assert_eq!(p.kind(), PerspectiveKind::ThreePoint);
    assert_eq!(p.lines(viewport()).len(), 3 * 12 + 1);
}

#[test]
fn one_point_horizon_is_horizontal_through_the_point() {
    let vp = Vec2::new(40.0, 77.0);
    let (point, dir) = Perspective::one_point(vp).horizon();
    assert!(close_pt(point, vp));
    assert!(close(dir.y, 0.0));
    assert!(close(dir.x.abs(), 1.0));
}

#[test]
fn set_point_moves_a_vanishing_point_and_rejects_bad_indices() {
    let mut p = Perspective::two_point(Vec2::new(10.0, 10.0), Vec2::new(20.0, 20.0));
    assert!(p.set_point(1, Vec2::new(30.0, 40.0)));
    assert!(close_pt(p.points()[1], Vec2::new(30.0, 40.0)));
    assert!(!p.set_point(2, Vec2::ZERO));
}

#[test]
fn radial_symmetry_gives_n_copies_in_both_modes() {
    let c = Vec2::new(250.0, 150.0);
    for mirrored in [false, true] {
        let sym = Symmetry::new(
            SymmetryKind::Radial {
                segments: 6,
                mirrored,
            },
            c,
        );
        assert_eq!(sym.copy_count(), 6);
        let copies = sym.transform(Vec2::new(300.0, 170.0));
        assert_eq!(copies.len(), 6);
        assert!(close_pt(copies[0], Vec2::new(300.0, 170.0)));
    }
}

#[test]
fn rotational_radial_rotates_by_whole_segments() {
    let c = Vec2::new(0.0, 0.0);
    let sym = Symmetry::new(
        SymmetryKind::Radial {
            segments: 8,
            mirrored: false,
        },
        c,
    );
    let copies = sym.transform(Vec2::new(10.0, 0.0));
    // Copy 2 is a 90-degree turn (2 * 2*pi/8), so (10, 0) -> (0, 10).
    assert!(close_pt(copies[2], Vec2::new(0.0, 10.0)));
    assert!(close_pt(copies[4], Vec2::new(-10.0, 0.0)));
}

#[test]
fn mirrored_radial_reflects_odd_copies_first() {
    let sym = Symmetry::new(
        SymmetryKind::Radial {
            segments: 4,
            mirrored: true,
        },
        Vec2::ZERO,
    );
    // d = (10, 4). Copy 1 = rotate90(reflect_x(d)) = rotate90((10, -4)) = (4, 10).
    let copies = sym.transform(Vec2::new(10.0, 4.0));
    assert!(close_pt(copies[1], Vec2::new(4.0, 10.0)));
}

#[test]
fn mirrored_radial_with_two_segments_is_vertical_mirror() {
    let c = Vec2::new(100.0, 100.0);
    let radial = Symmetry::new(
        SymmetryKind::Radial {
            segments: 2,
            mirrored: true,
        },
        c,
    );
    let vertical = Symmetry::new(SymmetryKind::Vertical, c);
    let p = Vec2::new(130.0, 90.0);
    let (a, b) = (radial.transform(p), vertical.transform(p));
    assert!(close_pt(a[1], b[1]));
}

#[test]
fn vertical_and_horizontal_give_two_mirrored_copies() {
    let c = Vec2::new(50.0, 60.0);
    let v = Symmetry::new(SymmetryKind::Vertical, c).transform(Vec2::new(60.0, 70.0));
    assert!(close_pt(v[1], Vec2::new(40.0, 70.0)));
    let h = Symmetry::new(SymmetryKind::Horizontal, c).transform(Vec2::new(60.0, 70.0));
    assert!(close_pt(h[1], Vec2::new(60.0, 50.0)));
}

#[test]
fn quadrant_mirrored_and_rotational_have_four_copies() {
    let c = Vec2::new(0.0, 0.0);
    let p = Vec2::new(3.0, 5.0);
    let mirrored = Symmetry::new(SymmetryKind::Quadrant { mirrored: true }, c).transform(p);
    for want in [
        Vec2::new(3.0, 5.0),
        Vec2::new(-3.0, 5.0),
        Vec2::new(3.0, -5.0),
        Vec2::new(-3.0, -5.0),
    ] {
        assert!(
            mirrored.iter().any(|q| close_pt(*q, want)),
            "missing {want:?}"
        );
    }
    let turned = Symmetry::new(SymmetryKind::Quadrant { mirrored: false }, c).transform(p);
    assert_eq!(turned.len(), 4);
    assert!(close_pt(turned[2], Vec2::new(-3.0, -5.0)));
}

#[test]
fn symmetry_apply_keeps_pressure_and_stroke_lengths() {
    let sym = Symmetry::new(
        SymmetryKind::Radial {
            segments: 3,
            mirrored: false,
        },
        Vec2::ZERO,
    );
    let samples = [
        Sample::new(Vec2::new(10.0, 0.0), 0.2),
        Sample::new(Vec2::new(0.0, 10.0), 0.9),
    ];
    let strokes = sym.apply(&samples);
    assert_eq!(strokes.len(), 3);
    for stroke in &strokes {
        assert_eq!(stroke.len(), 2);
    }
    assert!(strokes[1]
        .iter()
        .zip(samples)
        .all(|(a, b)| a.pressure == b.pressure));
}

#[test]
fn symmetry_guide_lines_cover_axes() {
    let c = Vec2::new(250.0, 150.0);
    assert_eq!(
        Symmetry::new(SymmetryKind::Vertical, c)
            .lines(viewport())
            .len(),
        1
    );
    assert_eq!(
        Symmetry::new(SymmetryKind::Quadrant { mirrored: true }, c)
            .lines(viewport())
            .len(),
        2
    );
    let radial = Symmetry::new(
        SymmetryKind::Radial {
            segments: 6,
            mirrored: true,
        },
        c,
    );
    assert_eq!(radial.lines(viewport()).len(), 6);
    // Mirror axes are at multiples of PI/N, so each line is at a known angle.
    for seg in radial.lines(viewport()) {
        let dir = direction_deg(&seg);
        let step = 180.0 / 6.0;
        let k = (dir / step).round();
        assert!((dir - k * step).abs() < 1e-6);
    }
}
