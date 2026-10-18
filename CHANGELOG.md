# Changelog

All notable changes to Eskiz are listed here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses [Semantic Versioning](https://semver.org/).

## [0.1.0] - Unreleased

First public preview of Eskiz, a free, open source painting app for macOS, Windows, Linux and (experimentally) Android.

### In short

- **Paint and draw** with dozens of brushes, pen pressure and tilt (Wacom-style tablets, and an iPad with Apple Pencil through Sidecar on a Mac), smoothing, smudge, eraser and fill.
- **Layers** with blend modes, masks and clipping; **selections**, **transform** and **adjustments** such as blur, curves and hue.
- **Text**, **drawing guides** (grid, perspective, symmetry), simple **frame-by-frame animation** (export as GIF or APNG), and painting on **3D models**.
- **Opens and saves `.procreate` files**, and exports PNG, JPEG, PSD, PDF and more.
- **Free, no account, no ads, no data collected.** Your drawings stay on your computer.
- **New here?** A short welcome tour shows the basics; replay it any time from Actions > Help > Welcome tour.

**How to install:** pick the file for your computer under **Assets** below and follow the steps in the [README](https://github.com/evarnimtb-netizen/eskiz-app#download-and-install). The apps are not code-signed, so your system asks you to confirm the first time you open them; that is expected.

**Found a bug?** [Report it here](https://github.com/evarnimtb-netizen/eskiz-app/issues/new?template=bug_report.yml). It helps a lot.

The full list of changes follows.

### Added

- A skippable welcome tour on first launch: a sample canvas and three coach marks (paint, undo, brush size). Replay it from Actions > Help > Welcome tour.
- Painting with a pressure, tilt and velocity aware brush engine: smoothing and stabilization, taper, jitter, grain and shape textures, wet mixing, smudge, eraser, an eyedropper, Pour Fill and Hold to Shape.
- A brush library with live previews, a brush bench for editing brush settings, and import of `.brush`, `.brushset` and `.abr` files.
- Layers with groups, masks, clipping, lock transparency, 27 blend modes, per-layer opacity, hide and lock, a background color, and 8-bit or 16-bit color depth with sRGB and Display P3 documents.
- Selections (automatic, freehand, rectangle and ellipse), saved selections, and Transform with freeform, uniform, distort and warp modes, snapping and numeric input; copy, cut and paste.
- Canvas operations: crop and resize, flip, canvas information, and a floating reference window.
- Adjustments with live preview and masks: Gaussian, motion and perspective blur, sharpen, noise, hue, saturation and brightness, color balance, curves, gradient map, bloom, chromatic aberration, glitch, halftone, clone and more, plus Liquify.
- Color tools: disc, classic and palette views, history, harmony, swatches (`.swatches`, `.aco`, `.ase`), and color palette extraction from pictures.
- Drawing guides (grid, isometric, perspective and symmetry) with Guided Layer.
- Text layers with fonts, runs, alignment, leading, justification and outlines, and import of your own fonts.
- Frame animation with onion skins and playback, GIF and APNG export, and a timelapse recorder with PNG sequence export.
- 3D model painting: import `.glb`, `.gltf`, `.obj` and USD models, paint on texture maps, and export glTF with textures.
- A gallery with stacks, a crash-recovery journal, gesture controls, an in-app user guide, and a diagnostic report (`eskiz --diagnose`).
- Opens and saves `.procreate` files for compatibility, reads and writes Photoshop PSD, and exports PNG, JPEG, TIFF, WebP and PDF.
- GPU-accelerated canvas and compositor with an automatic fallback to a software path when no suitable GPU is found.
- Packages: macOS universal DMG, Windows x64 zip, Linux AppImage and tarball, and an experimental Android APK. Every package includes the third-party license notices.

### Fixed (release-candidate smoke test)

- Turning on Settings > "Eskiz look" no longer quits the app (a heading font was used a frame before the new fonts were installed). Switching in either direction is safe at any moment, and so is starting with the look already on.
- The smudge tool only moves paint that is already there: the carried colour now thins out as it is laid down instead of lasting for ever, so it no longer paints over empty canvas like an airbrush.
- A stroke can no longer vanish while it is drawn: the eyedropper hold never cancels a stroke that has moved, and Hold to Shape puts your own stroke back if the shape would paint nothing.
- The Actions popover is compact on desktop (touch devices keep large targets); long row titles wrap or shorten instead of running under the switch, and subtitles wrap to two lines. "Eskiz look" has its own icon.
- The running app shows the Eskiz icon in the Dock, taskbar and window, not the default egui logo.
- The Actions popover is now as wide as its rows on macOS: a long pen/tablet status message in Settings no longer stretches the whole panel, and no popover can grow wider than its width or the window.
- Scrolling, trackpad swiping or pinching over a popover, panel, sheet or the side bar no longer also pans or zooms the canvas behind it.
- The Dock and Cmd-Tab icon is the right size on macOS: the app keeps its bundle icon, and the icon file uses Apple's icon grid (rounded body with margin and a soft shadow).
- User guide: the welcome tour is skipped for data folders that already hold artworks; Help > Welcome tour shows it.

### Fixed (second release-candidate smoke test)

- Strokes no longer vanish on a Mac with mouse or trackpad: eframe's 30-second periodic save cancelled a stroke that was open (and rolled the layer back) whenever the drawing had unsaved changes; it now waits for the pen-up. Pinch, scroll, Space and the Fit shortcut during a stroke no longer move the canvas under the pen, a second contact on the trackpad is ignored until the stroke ends, a lost pen-up (window focus lost or minimised, a press with the stroke still open) now finishes the stroke instead of dropping the next one, leaving for the Gallery, saving on exit and suspending commit an open stroke instead of cancelling it, and the GPU canvas refreshes the layer the stroke paints on even if the active layer changed. Set `ESKIZ_TRACE_STROKES=1` to log every stroke begin, commit and cancel with its reason (see Troubleshooting).
- The Eskiz look is calmer: neutral graphite surfaces with 1 px hairlines and no shadows, neutral slider fills, switches and selections, quieter icons, and the orange accent only on the active tool or tab and the one primary button of a screen. Classic is unchanged.
- The "Gallery" title and the canvas "Gallery" button have the same size, baseline and left edge, and on macOS both start clear of the window buttons.

### Fixed (third release-candidate smoke test: interface and canvas)

- Toasts ("Created Untitled copy", "Undo Stroke", ...) size to their text on one line up to 360 pt, wrap only between words beyond that, and never break inside a word; before, the anchored toast box was laid out at a sliver of a width.
- The side bar's grab handle and its redo icon now sit the same distance (about 11 pt) from the top and bottom edge of the pill, in both looks and in the right-hand interface.
- You can rotate the canvas on a Mac: twist two fingers on the trackpad, hold R and drag sideways (works with a pen or a mouse), Actions > Canvas > Rotate canvas 90 degrees left, Rotate canvas 90 degrees right and Reset rotation, or Cmd+Alt+Left, Cmd+Alt+Right and Cmd+Alt+0. The view sticks at 0, 90, 180 and 270 degrees (also for the two-finger touch gesture) and a toast shows the angle. Painting, selection, transform and the brush cursor follow the turned canvas.
- Hold to Shape tolerates a slippery pen: the hold fires when the pen stays within a few points over the hold time even if it jitters or creeps, the pen's wandering during the hold is cut from the stroke, and jitter is smoothed away before corners are counted, so a hand-drawn circle comes out as a Circle (or Ellipse) instead of a polygon while real rectangles, triangles, polygons and zigzags stay polygons and polylines.
- The first segment of a snapped open polyline no longer tapers to a hairline (shapes ignore the brush's start taper).

### Fixed (input, lag and Windows start-up)

- **Pen tablets on macOS.** Wacom-style tablets and an iPad with an Apple Pencil through Sidecar now give pressure, tilt, barrel rotation and the eraser end (before, the pen acted as a mouse with fixed pressure). Eskiz reads the tablet events macOS sends next to the mouse events; mouse and trackpad are unchanged. Settings > pressure curve shows the live pen readings, and `ESKIZ_TRACE_STROKES=1` logs them per stroke (see Troubleshooting).
- **Pour Fill no longer lags on big canvases.** Dragging sideways after the drop changes the threshold; the fill is now computed on a worker thread (newest request wins) and written into the layer only where the preview changed, so the window keeps drawing. The exact fill at the final threshold is made once when you let go. On a 4096 px canvas one step used to block the window for about 150 ms.
- **Moving or rotating a Hold to Shape shape no longer lags.** While you drag a handle (or the pen is still down) the shape is a light vector preview; it is painted into the layer once on release, still as one "Stroke" undo step. A frame used to take about 140 ms at 4096 px.
- **Windows: the program starts on a PC without the Visual C++ Redistributable.** eskiz.exe is built with the static C runtime and CI checks that it imports no VCRUNTIME, MSVCP or api-ms-win-crt DLL.
- **Windows: start-up failures are no longer silent.** Eskiz writes `startup.log` and `panic-*.txt` to `%LOCALAPPDATA%\Eskiz\logs` and shows a message box with the reason and that folder. A panic or error in the graphics start-up falls back to OpenGL, the chosen adapter and backend are logged, and if a graphics driver ended the process without a message, the next start tries DirectX 12 only, then OpenGL. `eskiz.exe --version`, `--selftest` and `--diagnose` print in the terminal that started them. README and release notes explain Extract All and the SmartScreen "More info, Run anyway" step.

### Known issues

- **Builds are not code-signed.** Your system will warn you the first time you open Eskiz.
  - macOS: Control-click Eskiz, choose Open, and confirm. If that does not work, open System Settings, Privacy & Security and choose Open Anyway, or run `xattr -dr com.apple.quarantine /Applications/Eskiz.app`.
  - Windows: extract the whole zip (Extract All) before running eskiz.exe. In the SmartScreen dialog, choose More info, then Run anyway.
  - Linux: make the AppImage executable with `chmod +x`.
  - Android: allow installing apps from your browser or file manager when asked.
- **Windows has not yet been tested on real hardware.** The Windows build passes the automated tests and self-test on GitHub's Windows runners only.
- **Android is experimental.** There is no system file picker, share sheet or clipboard integration yet, and only arm64 devices running Android 10 or newer are supported. The APK is signed with a public debug key.
- **MP4 export is not included.** It needs the OpenH264 codec, which these builds do not bundle. GIF, APNG and PNG sequence export work. Developers can build with `--features mp4`.
- Some adjustment defaults and several 3D and Liquify details are approximations and may change.

[0.1.0]: https://github.com/evarnimtb-netizen/eskiz-app/releases/tag/v0.1.0
