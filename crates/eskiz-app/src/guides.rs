//! Drawing guides and Guided Layer.
//!
//! * [`GuideSettings`] is the guide the user picked and how it looks. It belongs to one
//!   document, so [`GuideState`] saves it in `<library>/<id>.guides`. The `.procreate`
//!   writer has no field for it.
//! * Guided Layer is switched on per layer ([`GuideState::set_assist`]). Such a layer's
//!   strokes are snapped to the grid, isometric or perspective guide, or, with the
//!   symmetry guide, mirrored into N strokes that paint as one undo step.
//! * Guide lines come from `eskiz-guides` in canvas space. The window draws them through
//!   the view transform (see `guide_ui`).

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use eskiz_core::geometry::Sample as CoreSample;
use eskiz_guides::{
    DrawingAssist, Grid, GuideRender, GuideStyle, Isometric, Perspective, Rect as GRect,
    Sample as GuideSample, SnapGuide, StyledSegment, Symmetry, SymmetryKind, Vec2,
    DEFAULT_LOCK_DISTANCE,
};

/// The guide type picked in the editor. Symmetry mirrors strokes; the others snap them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuideKind {
    Grid,
    Isometric,
    Perspective,
    Symmetry,
}

impl GuideKind {
    pub const ALL: [GuideKind; 4] = [
        Self::Grid,
        Self::Isometric,
        Self::Perspective,
        Self::Symmetry,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Grid => "2D grid",
            Self::Isometric => "Isometric",
            Self::Perspective => "Perspective",
            Self::Symmetry => "Symmetry",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Grid => "grid",
            Self::Isometric => "isometric",
            Self::Perspective => "perspective",
            Self::Symmetry => "symmetry",
        }
    }

    fn from_key(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.key() == s)
    }
}

/// The mirror layout of the symmetry guide.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymMode {
    Vertical,
    Horizontal,
    Quadrant,
    Radial,
}

impl SymMode {
    pub const ALL: [SymMode; 4] = [
        Self::Vertical,
        Self::Horizontal,
        Self::Quadrant,
        Self::Radial,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Vertical => "Vertical",
            Self::Horizontal => "Horizontal",
            Self::Quadrant => "Quadrant",
            Self::Radial => "Radial",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Vertical => "vertical",
            Self::Horizontal => "horizontal",
            Self::Quadrant => "quadrant",
            Self::Radial => "radial",
        }
    }

    fn from_key(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.key() == s)
    }
}

/// Guide settings for one document. Positions are fractions of the canvas, so they keep
/// their place when the canvas is resized.
#[derive(Clone, Debug, PartialEq)]
pub struct GuideSettings {
    /// The "Drawing guide" toggle in Actions, Canvas.
    pub visible: bool,
    pub kind: GuideKind,
    /// 0 to 1.
    pub opacity: f64,
    /// Canvas pixels.
    pub thickness: f64,
    /// Grid and isometric spacing, canvas pixels.
    pub grid_size: f64,
    pub colour: [u8; 3],
    /// Perspective: 1, 2 or 3 vanishing points.
    pub perspective_points: usize,
    /// Vanishing points, as canvas fractions.
    pub vanishing: [Vec2; 3],
    pub sym: SymMode,
    pub radial_segments: u32,
    /// Rotate copies instead of mirroring them.
    pub rotational: bool,
    /// Symmetry centre, as a canvas fraction.
    pub centre: Vec2,
    /// Drag distance before an isometric or perspective stroke locks its direction.
    pub lock_distance: f64,
}

impl Default for GuideSettings {
    fn default() -> Self {
        Self {
            visible: false,
            kind: GuideKind::Grid,
            opacity: 0.5,
            thickness: 1.0,
            grid_size: 64.0,
            colour: [70, 130, 220],
            perspective_points: 2,
            vanishing: [
                Vec2::new(0.5, 0.5),
                Vec2::new(-0.5, 0.4),
                Vec2::new(0.5, -1.0),
            ],
            sym: SymMode::Vertical,
            radial_segments: 6,
            rotational: false,
            centre: Vec2::new(0.5, 0.5),
            lock_distance: DEFAULT_LOCK_DISTANCE,
        }
    }
}

fn at_canvas(frac: Vec2, w: f64, h: f64) -> Vec2 {
    Vec2::new(frac.x * w, frac.y * h)
}

impl GuideSettings {
    fn style(&self) -> GuideStyle {
        GuideStyle::new(self.opacity, self.thickness)
    }

    fn perspective(&self, w: f64, h: f64) -> Perspective {
        let p = |i: usize| at_canvas(self.vanishing[i], w, h);
        let mut persp = match self.perspective_points {
            1 => Perspective::one_point(p(0)),
            3 => Perspective::three_point(p(0), p(1), p(2)),
            _ => Perspective::two_point(p(0), p(1)),
        };
        persp.style = self.style();
        persp
    }

    fn symmetry(&self, w: f64, h: f64) -> Symmetry {
        let mirrored = !self.rotational;
        let kind = match self.sym {
            SymMode::Vertical => SymmetryKind::Vertical,
            SymMode::Horizontal => SymmetryKind::Horizontal,
            SymMode::Quadrant => SymmetryKind::Quadrant { mirrored },
            SymMode::Radial => SymmetryKind::Radial {
                segments: self.radial_segments,
                mirrored,
            },
        };
        let mut sym = Symmetry::new(kind, at_canvas(self.centre, w, h));
        sym.style = self.style();
        sym
    }

    /// Guide lines for a `w` by `h` canvas, clipped to `viewport` (canvas space).
    pub fn lines(&self, w: f64, h: f64, viewport: GRect) -> Vec<StyledSegment> {
        let style = self.style();
        match self.kind {
            GuideKind::Grid => {
                let mut g = Grid::new(self.grid_size);
                g.style = style;
                g.render(viewport)
            }
            GuideKind::Isometric => {
                let mut g = Isometric::new(self.grid_size);
                g.style = style;
                g.render(viewport)
            }
            GuideKind::Perspective => self.perspective(w, h).render(viewport),
            GuideKind::Symmetry => self.symmetry(w, h).render(viewport),
        }
    }

    /// The Guided Layer settings for a stroke on a `w` by `h` canvas. Grid, isometric and
    /// perspective guides snap; the symmetry guide mirrors.
    pub fn assist(&self, w: f64, h: f64) -> DrawingAssist {
        let snap = match self.kind {
            GuideKind::Grid => Some(SnapGuide::Grid(Grid::new(self.grid_size))),
            GuideKind::Isometric => Some(SnapGuide::Isometric(Isometric::new(self.grid_size))),
            GuideKind::Perspective => Some(SnapGuide::Perspective(self.perspective(w, h))),
            GuideKind::Symmetry => None,
        };
        let symmetry = (self.kind == GuideKind::Symmetry).then(|| self.symmetry(w, h));
        DrawingAssist {
            snap,
            symmetry,
            lock_distance: self.lock_distance,
        }
    }

    fn encode(&self) -> String {
        let v = |p: Vec2| format!("{},{}", p.x, p.y);
        let [r, g, b] = self.colour;
        format!(
            "visible={}\nkind={}\nopacity={}\nthickness={}\ngrid={}\ncolour={r},{g},{b}\n\
             perspective={}\nvanishing={};{};{}\nsym={}\nsegments={}\nrotational={}\ncentre={}\nlock={}\n",
            u8::from(self.visible),
            self.kind.key(),
            self.opacity,
            self.thickness,
            self.grid_size,
            self.perspective_points,
            v(self.vanishing[0]),
            v(self.vanishing[1]),
            v(self.vanishing[2]),
            self.sym.key(),
            self.radial_segments,
            u8::from(self.rotational),
            v(self.centre),
            self.lock_distance,
        )
    }

    /// Applies one `key=value` line. Unknown keys and bad values keep the current setting.
    fn apply_line(&mut self, key: &str, value: &str) {
        fn num(s: &str) -> Option<f64> {
            s.trim().parse::<f64>().ok().filter(|v| v.is_finite())
        }
        fn point(s: &str) -> Option<Vec2> {
            let (x, y) = s.split_once(',')?;
            Some(Vec2::new(num(x)?, num(y)?))
        }
        match key {
            "visible" => self.visible = value.trim() == "1",
            "kind" => self.kind = GuideKind::from_key(value.trim()).unwrap_or(self.kind),
            "opacity" => self.opacity = num(value).map_or(self.opacity, |v| v.clamp(0.0, 1.0)),
            "thickness" => {
                self.thickness = num(value).map_or(self.thickness, |v| v.clamp(0.1, 32.0))
            }
            "grid" => self.grid_size = num(value).map_or(self.grid_size, |v| v.clamp(2.0, 1024.0)),
            "colour" => {
                let parts: Vec<u8> = value
                    .split(',')
                    .filter_map(|p| p.trim().parse().ok())
                    .collect();
                if let [r, g, b] = parts[..] {
                    self.colour = [r, g, b];
                }
            }
            "perspective" => {
                self.perspective_points = value
                    .trim()
                    .parse::<usize>()
                    .map_or(self.perspective_points, |n| n.clamp(1, 3));
            }
            "vanishing" => {
                let pts: Vec<Vec2> = value.split(';').filter_map(point).collect();
                for (slot, p) in self.vanishing.iter_mut().zip(pts) {
                    *slot = p;
                }
            }
            "sym" => self.sym = SymMode::from_key(value.trim()).unwrap_or(self.sym),
            "segments" => {
                self.radial_segments = value
                    .trim()
                    .parse::<u32>()
                    .map_or(self.radial_segments, |n| n.clamp(2, 32));
            }
            "rotational" => self.rotational = value.trim() == "1",
            "centre" => self.centre = point(value).unwrap_or(self.centre),
            "lock" => {
                self.lock_distance = num(value).map_or(self.lock_distance, |v| v.clamp(0.0, 512.0))
            }
            _ => {}
        }
    }
}

/// Converts a stroke sample to the sample type the assist works on.
pub fn to_guide_sample(s: &CoreSample) -> GuideSample {
    GuideSample::new(Vec2::new(s.x, s.y), s.pressure.unwrap_or(1.0))
}

/// Rebuilds one sample per guided output: each keeps the input's tilt, time and pressure
/// and takes the guided position.
pub fn to_core_samples(base: &CoreSample, guided: &[GuideSample]) -> Vec<CoreSample> {
    guided
        .iter()
        .map(|g| CoreSample {
            x: g.pos.x,
            y: g.pos.y,
            ..*base
        })
        .collect()
}

/// Guide state the app keeps: the settings of the open document, which layers have Drawing
/// Assist, and the editor's UI state.
#[derive(Debug, Default)]
pub struct GuideState {
    pub settings: GuideSettings,
    /// Layers (by id) with Guided Layer on.
    pub assist_layers: BTreeSet<u64>,
    /// Index into the harmony modes (see the Colour panel). Not saved.
    pub harmony: usize,
    /// Whether the guide editor window is open. Not saved.
    pub editor_open: bool,
    /// The "Palette files" window and its path. Not saved.
    pub palette_window_open: bool,
    pub palette_path: String,
    doc_key: Option<String>,
    /// The text last read from or written to the document's guide file.
    saved: Option<String>,
}

impl GuideState {
    /// Follows the open document. `key` is its library ID (`None` for an unsaved document)
    /// and `dir` is the library folder. Switching documents loads that document's guides,
    /// or the defaults when it has none. `assigned` is the ID the latest first save gave a new
    /// document: only that document keeps the guides it had before it was saved.
    pub fn sync(&mut self, key: Option<&str>, dir: Option<&Path>, assigned: Option<&str>) {
        if key.map(str::to_owned) != self.doc_key {
            // An external document (opened from a file) also has no key until its first save:
            // it must not inherit the guides of the drawing before it.
            let was_unsaved = self.doc_key.is_none() && key.is_some() && key == assigned;
            self.doc_key = key.map(str::to_owned);
            match (key, dir) {
                (Some(id), Some(dir)) => {
                    if let Some((settings, layers)) = fs::read_to_string(file_for(dir, id))
                        .ok()
                        .and_then(|t| decode(&t))
                    {
                        self.settings = settings;
                        self.assist_layers = layers;
                        self.saved = Some(self.encode());
                    } else if was_unsaved {
                        // A document that has just received its first save keeps what was
                        // set on it, and the file is written below.
                        self.saved = None;
                    } else {
                        self.reset();
                    }
                }
                _ => self.reset(),
            }
        }
        self.persist(dir);
    }

    fn reset(&mut self) {
        self.settings = GuideSettings::default();
        self.assist_layers.clear();
        self.saved = None;
    }

    fn encode(&self) -> String {
        let layers: Vec<String> = self.assist_layers.iter().map(u64::to_string).collect();
        format!(
            "eskiz-guides 1\n{}layers={}\n",
            self.settings.encode(),
            layers.join(",")
        )
    }

    /// Writes the settings when they differ from the file. Errors are ignored: guides are
    /// a convenience, and a failed write must not stop painting. A failed write is not tried
    /// again every frame; the next change tries again.
    fn persist(&mut self, dir: Option<&Path>) {
        let (Some(id), Some(dir)) = (self.doc_key.clone(), dir) else {
            return;
        };
        let text = self.encode();
        if self.saved.as_deref() == Some(text.as_str()) {
            return;
        }
        let _ = crate::storage::write_atomic(&file_for(dir, &id), text.as_bytes());
        self.saved = Some(text);
    }

    pub fn assist_on(&self, layer: u64) -> bool {
        self.assist_layers.contains(&layer)
    }

    /// Switches Guided Layer on or off for one layer.
    pub fn set_assist(&mut self, layer: u64, on: bool) {
        if on {
            self.assist_layers.insert(layer);
        } else {
            self.assist_layers.remove(&layer);
        }
    }

    /// The assist for a stroke on `layer`, or `None` when the layer has none or the
    /// settings would neither snap nor mirror.
    pub fn stroke_assist(&self, layer: u64, w: usize, h: usize) -> Option<DrawingAssist> {
        if !self.assist_on(layer) {
            return None;
        }
        let assist = self.settings.assist(w as f64, h as f64);
        (assist.snap.is_some() || assist.symmetry.is_some()).then_some(assist)
    }
}

fn file_for(dir: &Path, id: &str) -> std::path::PathBuf {
    dir.join(format!("{id}.guides"))
}

/// Parses a guide file. `None` when the header is missing.
fn decode(text: &str) -> Option<(GuideSettings, BTreeSet<u64>)> {
    let mut lines = text.lines();
    // The header of files written before the rename is still accepted.
    if !matches!(lines.next()?.trim(), "eskiz-guides 1" | "easel-guides 1" /* legacy-easel */) {
        return None;
    }
    let mut settings = GuideSettings::default();
    let mut layers = BTreeSet::new();
    for line in lines {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key == "layers" {
            layers = value
                .split(',')
                .filter_map(|s| s.trim().parse::<u64>().ok())
                .collect();
        } else {
            settings.apply_line(key.trim(), value);
        }
    }
    Some((settings, layers))
}

use crate::input::{sample_from, PointerKind, RawPointer};
use crate::paint::StrokeSettings;
use crate::session::Session;
use eskiz_core::brushes::{Brush, Preset};
use eskiz_core::document::Document;
use eskiz_guides::{Isometric as IsoGuide, Symmetry as Sym, SymmetryKind as SK};

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("eskiz-guides-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample_settings() -> GuideSettings {
        GuideSettings {
            visible: true,
            kind: GuideKind::Perspective,
            opacity: 0.37,
            thickness: 2.5,
            grid_size: 96.0,
            colour: [12, 200, 99],
            perspective_points: 3,
            vanishing: [
                Vec2::new(0.1, 0.2),
                Vec2::new(1.3, -0.4),
                Vec2::new(0.5, 2.0),
            ],
            sym: SymMode::Radial,
            radial_segments: 8,
            rotational: true,
            centre: Vec2::new(0.25, 0.75),
            lock_distance: 22.0,
        }
    }

    #[test]
    fn guide_settings_and_assist_layers_round_trip_through_the_file() {
        let dir = temp_dir("round-trip");
        let mut state = GuideState {
            settings: sample_settings(),
            ..GuideState::default()
        };
        state.set_assist(7, true);
        state.set_assist(42, true);
        state.sync(Some("doc-a"), Some(&dir), Some("doc-a"));

        let mut reopened = GuideState::default();
        reopened.sync(Some("doc-a"), Some(&dir), None);
        assert_eq!(reopened.settings, state.settings);
        assert_eq!(reopened.assist_layers, BTreeSet::from([7, 42]));
        // Switching to a document with no file gives the defaults.
        reopened.sync(Some("doc-b"), Some(&dir), None);
        assert_eq!(reopened.settings, GuideSettings::default());
        assert!(reopened.assist_layers.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_guide_file_with_the_old_header_still_reads() {
        let old = "easel-guides 1\nopacity=0.5\nlayers=4,7\n"; // legacy-easel
        let (settings, layers) = decode(old).unwrap();
        assert_eq!(settings.opacity, 0.5);
        assert_eq!(layers, BTreeSet::from([4, 7]));
        let rewritten = format!("eskiz-guides 1\n{}layers=4,7\n", settings.encode());
        assert_eq!(decode(&rewritten).unwrap().1, layers);
    }

    #[test]
    fn a_missing_or_damaged_file_gives_defaults() {
        assert!(decode("").is_none());
        assert!(decode("not a guide file\nopacity=0.2\n").is_none());
        let (settings, layers) =
            decode("eskiz-guides 1\nopacity=9\nkind=spiral\nlayers=x,3\n").unwrap();
        assert_eq!(settings.opacity, 1.0, "opacity is clamped");
        assert_eq!(
            settings.kind,
            GuideKind::Grid,
            "unknown kind keeps the default"
        );
        assert_eq!(layers, BTreeSet::from([3]));
    }

    #[test]
    fn assist_is_per_layer() {
        let mut state = GuideState::default();
        state.settings.kind = GuideKind::Symmetry;
        state.set_assist(5, true);
        assert!(state
            .stroke_assist(5, 300, 300)
            .is_some_and(|a| a.output_count() == 2));
        assert!(state.stroke_assist(6, 300, 300).is_none());
    }

    #[test]
    fn symmetry_gives_one_output_stroke_per_copy() {
        let assist = DrawingAssist {
            snap: None,
            symmetry: Some(Sym::new(
                SK::Radial {
                    segments: 4,
                    mirrored: false,
                },
                Vec2::new(0.0, 0.0),
            )),
            lock_distance: DEFAULT_LOCK_DISTANCE,
        };
        assert_eq!(assist.output_count(), 4);
        let samples: Vec<GuideSample> = (0..5)
            .map(|i| GuideSample::new(Vec2::new(10.0 + i as f64, 3.0), 1.0))
            .collect();
        let strokes = assist.assist(&samples);
        assert_eq!(strokes.len(), 4);
        assert!(strokes.iter().all(|s| s.len() == samples.len()));
        // The owned stroke gives the same output as the borrowed one.
        let mut owned = assist.begin_stroke_owned();
        let mut borrowed = assist.begin_stroke();
        for s in &samples {
            assert_eq!(owned.push(*s), borrowed.push(*s));
        }
    }
}

// Headless checks. They run under --selftest as well as in the tests below.
fn paint_settings(size: f64) -> StrokeSettings {
    let mut brush = Brush::preset(Preset::InkWithTaper);
    brush.size = size;
    StrokeSettings {
        brush,
        colour: [0.0, 0.0, 0.0],
        erase: false,
        raster: None,
    }
}

fn drag(
    session: &mut Session,
    layer: u64,
    assist: &DrawingAssist,
    pts: &[Vec2],
) -> Result<(), String> {
    drag_with(session, layer, &paint_settings(12.0), assist, pts)
}

fn drag_with(
    session: &mut Session,
    layer: u64,
    settings: &StrokeSettings,
    assist: &DrawingAssist,
    pts: &[Vec2],
) -> Result<(), String> {
    if !session.begin_assisted_stroke(layer, settings, assist) {
        return Err("the assisted stroke did not start".into());
    }
    for (i, p) in pts.iter().enumerate() {
        let raw = RawPointer {
            x: p.x,
            y: p.y,
            time: i as f64 / 120.0,
            kind: PointerKind::Mouse,
            force: None,
        };
        session.push(sample_from(raw));
    }
    session
        .end_stroke()
        .map(|_| ())
        .ok_or_else(|| "the stroke painted nothing".to_owned())
}

/// Whether ink lands within three pixels of `p`. Grain leaves single-pixel gaps in a
/// stroke, so a single pixel is not a fair test.
fn painted(session: &Session, layer: u64, p: Vec2) -> bool {
    let pixels = session.doc.layer(layer).unwrap();
    let (cx, cy) = (p.x.round() as i64, p.y.round() as i64);
    (-3..=3).any(|dy| {
        (-3..=3).any(|dx| {
            let (x, y) = ((cx + dx) as usize, (cy + dy) as usize);
            pixels.pixel(x, y)[3] > 0
        })
    })
}

/// A radial-4 mirror of one stroke paints four strokes, as one undo step.
fn check_radial_four() -> Result<String, String> {
    let mut s = Session::new(Document::new(300, 300));
    let layer = s.doc.layer_id(0).ok_or("no layer")?;
    let centre = Vec2::new(150.0, 150.0);
    let assist = DrawingAssist {
        snap: None,
        symmetry: Some(Sym::new(
            SK::Radial {
                segments: 4,
                mirrored: false,
            },
            centre,
        )),
        lock_distance: DEFAULT_LOCK_DISTANCE,
    };
    let steps = s.undo_steps();
    // An arm from 10 to 60 px from the centre. Copies are checked at 35 px, mid-stroke,
    // where the taper has not thinned the stroke.
    let arm: Vec<Vec2> = (0..=20)
        .map(|i| Vec2::new(160.0 + i as f64 * 2.5, 150.0))
        .collect();
    drag(&mut s, layer, &assist, &arm)?;
    if s.undo_steps() != steps + 1 {
        return Err(format!(
            "radial-4 stroke made {} undo steps, expected 1",
            s.undo_steps() - steps
        ));
    }
    let ends = [
        Vec2::new(185.0, 150.0),
        Vec2::new(150.0, 185.0),
        Vec2::new(115.0, 150.0),
        Vec2::new(150.0, 115.0),
    ];
    for end in ends {
        if !painted(&s, layer, end) {
            return Err(format!("radial-4: no paint at arm end {end:?}"));
        }
    }
    s.undo().ok_or("undo refused")?;
    if ends.iter().any(|&e| painted(&s, layer, e)) {
        return Err("radial-4: undo left paint behind".into());
    }
    Ok("  guided layer: radial-4 symmetry stroke paints 4 arms in one undo step, undo clears all".to_owned())
}

/// An isometric-assisted stroke that is dragged off-axis lands on an isometric axis.
fn check_isometric_lands_on_axis() -> Result<String, String> {
    let iso = IsoGuide::new(20.0);
    let anchor = Vec2::new(60.0, 200.0);
    let angle = 50f64.to_radians();
    let dir = Vec2::from_angle(angle);
    let assist = DrawingAssist {
        snap: Some(SnapGuide::Isometric(IsoGuide::new(20.0))),
        symmetry: None,
        lock_distance: DEFAULT_LOCK_DISTANCE,
    };
    let pts: Vec<Vec2> = (0..=20).map(|i| anchor + dir * (i as f64 * 4.0)).collect();

    // Geometry: every guided point after the lock sits on one isometric axis through the anchor.
    let guided = assist.assist(
        &pts.iter()
            .map(|p| GuideSample::new(*p, 1.0))
            .collect::<Vec<_>>(),
    );
    let last = guided[0].last().ok_or("no guided output")?.pos;
    let drag = last - anchor;
    let on_axis = iso.axes().iter().any(|a| {
        let off = a.x * (last.y - anchor.y) - a.y * (last.x - anchor.x);
        off.abs() < 1e-6 * drag.length().max(1.0)
    });
    if !on_axis {
        return Err(format!(
            "isometric: guided end {last:?} is not on an axis through the anchor"
        ));
    }

    // Paint, checked halfway along: the snapped point is on the axis, and the raw drag
    // point is well off it, so it stays unpainted.
    let mut s = Session::new(Document::new(300, 300));
    let layer = s.doc.layer_id(0).ok_or("no layer")?;
    drag_points(&mut s, layer, &assist, &pts)?;
    let snapped = guided[0][10].pos;
    if !painted(&s, layer, snapped) {
        return Err(format!(
            "isometric: no paint at the snapped point {snapped:?}"
        ));
    }
    let free = pts[10];
    if painted(&s, layer, free) {
        return Err("isometric: paint at the unsnapped point, so the stroke did not snap".into());
    }
    Ok("  guided layer: an isometric-assisted stroke dragged at 50 degrees lands on an isometric axis".to_owned())
}

fn drag_points(
    session: &mut Session,
    layer: u64,
    assist: &DrawingAssist,
    pts: &[Vec2],
) -> Result<(), String> {
    drag(session, layer, assist, pts)
}

#[cfg(test)]
mod check_tests {
    use super::*;

    #[test]
    fn radial_four_check_passes() {
        check_radial_four().unwrap();
    }

    #[test]
    fn isometric_check_passes() {
        check_isometric_lands_on_axis().unwrap();
    }

    /// A radial-4 symmetric stroke painted with a textured library brush (tip and grain
    /// raster) paints four arms, and the grain shows in each: alpha varies inside every arm.
    #[test]
    fn radial_four_with_textured_library_brush_paints_four_textured_arms() {
        use eskiz_core::brushes::Grain;
        let lb = eskiz_brushlib::library_ref()
            .iter()
            .flat_map(|s| &s.brushes)
            .find(|b| b.grain.is_some() && b.brush.grain != Grain::None)
            .expect("a textured library brush");
        let engine = crate::brush_library::engine_for(lb);
        let mut brush = engine.brush.clone();
        brush.size = 70.0;
        brush.opacity = 1.0;
        let settings = StrokeSettings {
            brush,
            colour: [0.1, 0.2, 0.7],
            erase: false,
            raster: Some(engine.raster.clone()),
        };

        let mut s = Session::new(Document::new(400, 400));
        let layer = s.doc.layer_id(0).unwrap();
        let centre = Vec2::new(200.0, 200.0);
        let assist = DrawingAssist {
            snap: None,
            symmetry: Some(Sym::new(SK::Radial { segments: 4, mirrored: false }, centre)),
            lock_distance: DEFAULT_LOCK_DISTANCE,
        };
        let steps = s.undo_steps();
        // One arm along +x, 20 to 180 px from the centre.
        let arm: Vec<Vec2> = (0..=64)
            .map(|i| Vec2::new(220.0 + i as f64 * 2.5, 200.0))
            .collect();
        drag_with(&mut s, layer, &settings, &assist, &arm).unwrap();
        assert_eq!(s.undo_steps(), steps + 1, "one undo step for all four arms");

        let pixels = s.doc.layer(layer).unwrap();
        for k in 0..4 {
            let (sin, cos) = (std::f64::consts::FRAC_PI_2 * f64::from(k)).sin_cos();
            // The middle of the arm (60..140 px out), inside the brush width (|offset| < 20).
            let mut alphas = Vec::new();
            for r in (60..140).step_by(1) {
                for o in -20..=20 {
                    let (along, across) = (f64::from(r), f64::from(o));
                    let x = (centre.x + along * cos - across * sin).round() as usize;
                    let y = (centre.y + along * sin + across * cos).round() as usize;
                    alphas.push(f64::from(pixels.pixel(x, y)[3]));
                }
            }
            let n = alphas.len() as f64;
            let mean = alphas.iter().sum::<f64>() / n;
            let variance = alphas.iter().map(|a| (a - mean).powi(2)).sum::<f64>() / n;
            assert!(mean > 0.0, "arm {k} is empty");
            assert!(variance > 0.0, "arm {k} has flat alpha (variance {variance}): the grain is missing");
        }
    }

    /// Every path of a radial-4 stroke is clipped by the selection: a rectangular selection
    /// that cuts through all four arms leaves no ink outside it, and ink inside it.
    #[test]
    fn radial_four_stroke_inside_a_rectangular_selection_leaves_nothing_outside() {
        use eskiz_select::{raster, Rect, Selection};
        let (w, h) = (400usize, 400usize);
        let centre = Vec2::new(200.0, 200.0);
        let assist = DrawingAssist {
            snap: None,
            symmetry: Some(Sym::new(SK::Radial { segments: 4, mirrored: false }, centre)),
            lock_distance: DEFAULT_LOCK_DISTANCE,
        };
        // The arm is 20 to 180 px from the centre. The rectangle is off-centre, so all four
        // arms are cut at different lengths.
        let arm: Vec<Vec2> = (0..=64)
            .map(|i| Vec2::new(220.0 + i as f64 * 2.5, 200.0))
            .collect();
        let rect = (150usize, 130usize, 290usize, 260usize);
        let ink = |s: &Session, layer: u64| -> (usize, usize) {
            let pixels = s.doc.layer(layer).unwrap();
            let (mut inside, mut outside) = (0, 0);
            for y in 0..h {
                for x in 0..w {
                    if pixels.pixel(x, y)[3] > 0 {
                        if x >= rect.0 && x < rect.2 && y >= rect.1 && y < rect.3 {
                            inside += 1;
                        } else {
                            outside += 1;
                        }
                    }
                }
            }
            (inside, outside)
        };

        // Control: without a selection the stroke does reach outside the rectangle.
        let mut free = Session::new(Document::new(w, h));
        let layer = free.doc.layer_id(0).unwrap();
        drag(&mut free, layer, &assist, &arm).unwrap();
        assert!(ink(&free, layer).1 > 0, "control stroke must reach outside the rectangle");

        let mut s = Session::new(Document::new(w, h));
        let layer = s.doc.layer_id(0).unwrap();
        let mask = raster::rectangle(
            w,
            h,
            Rect::new(rect.0 as f32, rect.1 as f32, rect.2 as f32, rect.3 as f32),
        );
        s.set_selection("Select", Selection::active(mask)).expect("selection set");
        drag(&mut s, layer, &assist, &arm).unwrap();
        let (inside, outside) = ink(&s, layer);
        assert_eq!(outside, 0, "{outside} px painted outside the selection");
        assert!(inside > 0, "nothing painted inside the selection");
    }
}

/// Headless checks for `--selftest`.
pub fn selftest_checks() -> Result<Vec<String>, String> {
    Ok(vec![check_radial_four()?, check_isometric_lands_on_axis()?])
}
