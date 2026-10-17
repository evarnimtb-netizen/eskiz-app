# eskiz-guides

Drawing guides, Guided Layer and color maths for Eskiz. This crate is pure geometry
and color with no UI and no dependencies. The UI draws the segments it produces and
feeds stroke samples through the assist.

There is no detailed spec for guides. The behavior below comes from common
digital-painting practice, and each choice that is not dictated by the roadmap is
stated here.

## API

### Guides (`grid`, `perspective`, `symmetry`)

Every guide implements `GuideRender`:

```rust
trait GuideRender {
    fn style(&self) -> GuideStyle;                       // opacity 0..=1, thickness px
    fn lines(&self, viewport: Rect) -> Vec<Segment>;     // canvas space, clipped
    fn render(&self, viewport: Rect) -> Vec<StyledSegment>;
}
```

- `Grid { spacing, origin, rotation, style }`: a square grid. Use `Grid::new(spacing)`
  for an unrotated grid at the origin. `snap(p)` returns the nearest intersection.
- `Isometric { spacing, origin, rotation, style }`: three families at 30, 90 and 150
  degrees. `nearest_axis(dir)` and `snap_to_axis(anchor, p)` work on the axes.
- `Perspective`: `one_point`, `two_point` or `three_point` constructors.
  `set_point(i, p)` moves a vanishing point, and points may lie off the canvas.
  `horizon()` returns `(point, direction)`. `show_horizon` and `density` are public.
- `Symmetry { kind, center, style }` with `SymmetryKind::{Vertical, Horizontal,
  Quadrant { mirrored }, Radial { segments, mirrored }}`. `transform(p)` returns the
  copies of a point, and `apply(samples)` returns one stroke per copy.

### Guided Layer (`assist`)

```rust
let assist = DrawingAssist { snap: Some(SnapGuide::Perspective(p)), symmetry: Some(s), ..Default::default() };
let strokes: Vec<Vec<Sample>> = assist.assist(&samples);   // one Vec per output stroke
```

- `assist(samples)` gives `output_count()` strokes. That is the symmetry copy count,
  or 1 with no symmetry.
- For live drawing, `begin_stroke()` returns a `StrokeAssist`. Its
  `push(sample) -> Vec<Sample>` gives the same result as `assist` over the whole
  stroke, and the lock state persists between calls.

### Color (`color`, `disc`, `harmony`, `palette`, `colour_space`)

- `Rgb` (floats), `Rgb8` (8-bit, `FromStr` and `to_hex`), `Hsb` (alias `Hsv`), with
  `hsb_to_rgb` and `rgb_to_hsb`.
- `ColourDisc::{hit_test, point_for, hue_point, colour_at}`: UI disc geometry.
- `harmony(base, Harmony::{Complementary, SplitComplementary, Analogous, Triadic,
  Tetradic})` returns the companion colors.
- `ColourHistory`: newest first, deduplicated, capacity 10.
- `extract_palette(rgba_pixels, max_colours)`: k-means, capped at 30 colors.
- `ColourConverter::new(from, to)` with `apply` (clamped) and `apply_unclamped`, plus
  `convert`. Spaces are `ColorSpace::{Srgb, DisplayP3}`.

## Design choices

### Guides
- **Units.** Canvas pixels, y down. Angles are radians except in `ISOMETRIC_ANGLES_DEG`.
  A positive angle turns clockwise on screen.
- **Grid spacing** is the distance between neighboring lines. Both families are
  clipped to the viewport, and a line that only touches a corner is dropped.
- **Isometric spacing** is the perpendicular distance between lines of one family, so
  all three families have the same visual density.
- **Perspective fans.** Each vanishing point draws `density` undirected lines spaced
  evenly across 180 degrees (default 12, one every 15 degrees). A fan through a point
  inside the viewport always produces one segment per line.
- **Horizon.** With one point it is horizontal through that point. With two or three
  points it runs through the first two points, and is horizontal if they coincide.
  The third point of a three-point guide is free to place, and its fan is drawn like
  the others.
- **Symmetry lines.** Vertical and horizontal guides draw one axis each, and quadrant
  guides draw both. Radial guides draw mirror axes as full lines at multiples of pi/N
  when mirrored, and as rays at multiples of 2pi/N when rotational.
- **Line cap.** A single family is capped at 100,000 lines. Above that it draws
  nothing, which keeps a tiny spacing from freezing the UI.

### Symmetry
- **Copy 0 is always the input point**, so output stroke 0 is the stroke itself.
- **Vertical and horizontal** give 2 copies. **Quadrant** gives 4. **Radial** gives N.
- **Quadrant mirrored**: reflections across both axes. **Quadrant rotational**:
  rotations by 90, 180 and 270 degrees.
- **Radial rotational**: copy k is rotated by 2pi*k/N.
- **Radial mirrored**: copy k is first reflected across the horizontal axis when k is
  odd, then rotated by 2pi*k/N. With N = 2 this is a vertical mirror. For other N the
  copies form a kaleidoscope, not an exact dihedral group, which is the usual painting
  behavior.
- Pressure is copied unchanged to every copy.

### Guided Layer
- **Order.** Snapping runs first, then symmetry mirrors the snapped point. Copies are
  therefore exact mirrors of the snapped stroke, not of the raw input.
- **Grid.** Every sample moves to the nearest intersection. The grid does not lock, so
  a stroke becomes a row of lattice points. Consecutive duplicates are kept, so the
  sample count matches the input.
- **Isometric and perspective.** The stroke becomes a straight line through its first
  sample (the anchor). Until the drag reaches `lock_distance` (default 16 px), samples
  pass through unchanged. Then one direction is chosen and locked for the rest of the
  stroke:
  - Isometric: the nearest of the three axes to the drag. The sign is ignored.
  - Perspective: the vanishing point whose line from the anchor best matches the drag
    (undirected). Ties go to the earlier point.
  - If no direction exists, the stroke stays unsnapped. This happens when every
    vanishing point coincides with the anchor.
- **Determinism and stability.** The choice depends only on the first sample past the
  threshold, so the same input always gives the same output. Mid-stroke, the lock is
  never re-chosen, even when the cursor drifts toward another vanishing point.

### Color
- **HSB/HSV** are the same model. Hue is in degrees, and saturation and brightness are
  in `0..=1`. Achromatic colors have hue 0. The conversions round-trip to within 1e-9
  except where hue or saturation is undefined (black, or gray).
- **Hex** accepts `#RRGGBB`, `RRGGBB`, `#RGB` and `RGB`, in either case. Output is
  upper-case `#RRGGBB`.
- **Disc.** Hue is the polar angle, clockwise on screen. Inside the inner disc (70% of
  the outer radius by default), the radius is saturation: 0 at the center, 1 at the
  inner rim. Brightness is a separate slider, passed to `colour_at`. This is the
  "simpler acceptable mapping". The angle-as-hue-blend variant is not implemented.
- **Harmony offsets**: complementary +180; split complementary +150 and +210;
  analogous -30 and +30; triadic +120 and +240 (that is, plus and minus 120); tetradic
  +60, +180 and +240 (a rectangle). The companions keep the base's saturation and
  brightness, and the base is not included in the output.
- **History.** Newest first. Pushing a color that is already present moves it to the
  front, so the list never repeats a color.
- **Palette.** The pixels are RGBA. Pixels with alpha below 128 are ignored. If the
  image has no more distinct colors than the cap, those colors are returned exactly,
  most frequent first. Otherwise k-means runs with k-means++ seeding from a fixed
  SplitMix64 seed, so the result is deterministic. Clustering uses sRGB values, not
  Lab. Large images are subsampled to at most 20,000 points, and palettes are ordered
  by cluster population.
- **Display P3.** Both spaces use the sRGB transfer curve (IEC 61966-2-1), which is
  also the curve Display P3 uses. The transfer is odd-symmetric, so negative values from
  out-of-gamut results keep their sign. The 3x3 matrices are derived in code from the
  published primaries (sRGB per IEC 61966-2-1; Display P3 per SMPTE EG 432-1) and the
  D65 white point, with no chromatic adaptation. Deriving them keeps white exact: RGB
  (1,1,1) maps to D65 in both spaces. sRGB lies inside P3, so sRGB to P3 never clips.
  P3 to sRGB can leave `0..=1`, so `apply` clamps and `apply_unclamped` does not.

## Tests

`cargo test -p eskiz-guides` runs 49 tests in three files:

- `tests/guides.rs`: exact line counts for grid, perspective (one, two and three
  points, with and without horizon) and symmetry; isometric directions; rotation,
  mirror and copy-count rules; symmetry axes.
- `tests/assist.rs`: grid snapping (including a rotated grid), isometric and
  perspective locking, lock stability mid-stroke, streaming against whole-stroke
  results, radial and mirrored copies after snapping, and the identity case.
- `tests/colour.rs`: HSB and RGB round trips over a grid of values, hex parsing,
  disc round trips for both ring and inner disc, harmony angles, history rules,
  palette exactness and clustering, and Display P3 (the known sRGB red value,
  white and gray mapping, and round trips).

`cargo clippy -p eskiz-guides --all-targets -- -D warnings` is clean.

## Gaps

- **No UI.** Guide rendering, the disc's visual layout, handles for moving vanishing
  points, and per-layer enablement (roadmap 5.2) belong in the app.
- **Assist samples** use this crate's own `Sample` type. The app converts from
  `eskiz-core::Sample` at the boundary, and the crates are not coupled.
- **Isometric and perspective assist** produce straight strokes after the lock, with no
  curve or partial-lock mode. Grid snapping has no line lock.
- **Symmetry** does not support mirror axes at arbitrary angles, or offsets for
  mirrored quadrants. Radial mirroring for odd N is not an exact dihedral group.
- **Color management** stops at sRGB and Display P3 with 8-bit or float values. There
  are no 16-bit layers (roadmap 9.2), no ICC profiles (`qcms`/`lcms2`), and no LUTs.
- **Palette** works on RGBA slices. Image decoding is left to the caller. Clustering is
  in sRGB, not a perceptual space, and there is no weighting by saturation.
- **Harmony** offers only the five modes in the roadmap. Tetradic uses the rectangle
  form, since the square form is not planned.
- **Disc** does not implement the angle-as-hue-blend variant from the design.
  Brightness is a separate slider.

## Provenance

Built from common knowledge of painting guides and color science. No other product's code or
assets were used.
