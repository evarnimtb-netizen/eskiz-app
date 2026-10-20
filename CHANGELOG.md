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
- Layers with groups, masks, clipping, lock transparency, 26 blend modes, per-layer opacity, hide and lock, a background color, and 8-bit or 16-bit color depth with sRGB and Display P3 documents.
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
- Overlays and handles now work on a rotated canvas at any angle: the Crop & Resize rectangle and its handles follow the turned canvas and crop the right pixels, rectangle and ellipse selections are the box you draw on screen, and the text selection highlight and width handle turn with the view. Transform handles, Edit Shape handles, Liquify, Pour Fill, Pick Color and the drawing guides are covered by new tests at 0, 33, 90 and 180 degrees.

### Fixed (input, lag and Windows start-up)

- **Pen tablets on macOS.** Wacom-style tablets and an iPad with an Apple Pencil through Sidecar now give pressure, tilt, barrel rotation and the eraser end (before, the pen acted as a mouse with fixed pressure). Eskiz reads the tablet events macOS sends next to the mouse events; mouse and trackpad are unchanged. Settings > pressure curve shows the live pen readings, and `ESKIZ_TRACE_STROKES=1` logs them per stroke (see Troubleshooting).
- **Pour Fill no longer lags on big canvases.** Dragging sideways after the drop changes the threshold; the fill is now computed on a worker thread (newest request wins) and written into the layer only where the preview changed, so the window keeps drawing. The exact fill at the final threshold is made once when you let go. On a 4096 px canvas one step used to block the window for about 150 ms.
- **Moving or rotating a Hold to Shape shape no longer lags.** While you drag a handle (or the pen is still down) the shape is a light vector preview; it is painted into the layer once on release, still as one "Stroke" undo step. A frame used to take about 140 ms at 4096 px.
- **Windows: the program starts on a PC without the Visual C++ Redistributable.** eskiz.exe is built with the static C runtime and CI checks that it imports no VCRUNTIME, MSVCP or api-ms-win-crt DLL.
- **Windows: start-up failures are no longer silent.** Eskiz writes `startup.log` and `panic-*.txt` to `%LOCALAPPDATA%\Eskiz\logs` and shows a message box with the reason and that folder. A panic or error in the graphics start-up falls back to OpenGL, the chosen adapter and backend are logged, and if a graphics driver ended the process without a message, the next start tries DirectX 12 only, then OpenGL. `eskiz.exe --version`, `--selftest` and `--diagnose` print in the terminal that started them. README and release notes explain Extract All and the SmartScreen "More info, Run anyway" step.

### Fixed (Windows freeze: "Not Responding" after the first frames)

- No GPU wait can freeze the window any more. Readbacks and "wait until idle" on the UI thread were unbounded (`wait_indefinitely`); they now give up after 1.8 s. When a driver does not answer, Eskiz marks the GPU path unusable for the session, composites on the processor, logs it and shows "Graphics driver is slow; switched to compatible mode".
- A watchdog thread notices a window that has not finished a frame for 3 s. It writes a `STALL:` line with the phase (input, gallery, canvas composite, GPU readback, egui paint + present) to `startup.log` and leaves `stalled.marker`. The next start skips the renderer that froze (automatic, then DirectX 12, then Vulkan, then OpenGL) and says so in the log and in a one-time note. A stall while presenting first switches to a present mode that does not wait for the display.
- `eskiz.exe --renderer gl|dx12|vulkan|auto` (same as `ESKIZ_RENDERER`; `auto` forgets earlier stalls), `Eskiz (compatibility mode).bat` in the Windows zip, and Settings > Graphics > Compatibility graphics (OpenGL), applied on the next start. The log records what was requested and what was chosen.
- The Windows smoke test now reports a hung window ("Not Responding") and a stall marker, and also runs once with `--renderer gl`.
- Renderer ladder on every desktop system (Windows: DirectX 12, Vulkan, OpenGL, Software; macOS: Metal, OpenGL; Linux: Vulkan, wgpu OpenGL, OpenGL, Software). Crash and freeze detection now also work on macOS and Linux. After a start that crashed or froze, Eskiz starts in **safe graphics mode** one step lower, says so in a notice with **Try normal graphics**, and stays there until you choose it (before, a crash only moved the very next start).
- Settings > Graphics > Renderer: Automatic or a specific renderer. `ESKIZ_BACKEND` and `--renderer` take `auto`, `dx12`, `vulkan`, `metal`, `wgpu-gl`, `gl` and `software`; `ESKIZ_RENDERER` and `WGPU_BACKEND` keep working.
- `startup.log` lists every graphics adapter with vendor, device, driver version and backend, and the one chosen (OpenGL: vendor, renderer and version strings). A small driver blocklist (empty for now) can keep a known-bad driver off a renderer.

### Fixed (seventh release candidate)

- **Windows: closing the window saves your drawing first, then exits within about 2 s** (longer only while a save is still running). A slow close is no longer mistaken for a freeze, so the next start keeps your renderer.
- Text: a click at the end of a short word places the caret. The width knob now sits just outside the text box, so it can no longer be grabbed by accident.
- Android: Open, Save and Import use the system file picker, Share sends a picture to other apps, pictures can be copied and pasted, and the Back button closes panels before leaving the app.
- Files from other people are checked more carefully: damaged or oversized `.procreate`, PSD, brush and swatch files are refused with a message instead of exhausting memory.
- 3D documents are saved through a backup copy, so a failed save no longer loses the previous version.

### Fixed (eighth release candidate)

- **Windows: the window no longer freezes ("Not Responding") with DirectX 12.** The canvas no longer queues more drawing work than the graphics driver can finish; if frames still cannot be shown, Eskiz switches to drawing the canvas on the processor for that session instead of freezing, and remembers it for the next start.
- Android: the app now reaches its Java side (earlier builds showed "Method not found"). On an emulator, opening files, choosing a folder, imports, copying and pasting pictures, and the Back button work; saving into a chosen folder and Share are not checked on a device yet. Gallery > Import opens the system file picker directly.

### Fixed (ninth release candidate)

- **Windows: the window no longer freezes ("Not Responding") a few seconds after start on PCs with a pen or touch digitizer.** The pen tablet support (Windows Ink) now runs on its own thread and starts after the first frames, so it cannot stop the window; if it ever hangs, only pen input stops. It is only started when Windows reports a pen.
- New switches for the pen tablet: **Settings → Pen tablet input (Windows Ink)** (applies at once), `--no-tablet` and `ESKIZ_TABLET=off`. The compatibility-mode start file uses `--no-tablet`. If Eskiz froze while the pen was connected, the next start leaves the pen off, says so, and offers a **Turn pen input back on** button.

### Fixed (code review)

- Saving and crash recovery keep working if one save or recovery job fails; you now see a message instead of silence.
- Documents no longer drop out of their stacks or the gallery order when the library folder or one file is briefly unreadable (for example while a sync tool holds it). A damaged gallery order file falls back to the last good copy.
- Crash recovery restores what it can when a recovery file is missing, and tells you what was left out. Recovery and saved files are flushed to disk more safely.
- Exporting a drawing whose name contains a dot (such as `Sketch v1.2`) keeps the whole name, and Windows device names such as `CON` or `NUL` no longer break an export.
- Saved selections of one drawing no longer carry over into another drawing that you open next.
- A new drawing's timelapse is no longer cut when the drawing is first saved, and each run of the app records to its own folder. A failed frame or disk write no longer discards the recording so far.
- Two saves or exports to the same file at the same time can no longer mix their data.
- A 3D model with a looping node list no longer freezes the importer; it is refused with a short message.
- 3D painting now works on models whose texture coordinates go past the 0 to 1 square (tiled textures).
- Very large 3D textures are scaled down on import instead of using several gigabytes of memory.
- Animation exports (GIF, APNG, MP4) keep clipped layers inside their base layer, and held frames no longer copy the picture for every tick.
- Text with an enormous size, outline or scale is skipped instead of using all memory.
- Huge pictures (over 100 megapixels) are refused from the file header, and pictures larger than the screen's texture limit are shrunk in the Reference window.
- Brush packs, PSD files and documents that would expand to more memory than is reasonable are refused with a clear message.
- Crop and Resize Canvas refuses a size that is too large for the number of layers, instead of running out of memory.
- Groups that hold a layer with a blend mode other than Normal now look the same on screen as in exports and thumbnails.
- Curves keep pure white white.
- A failed start no longer makes the next start think Eskiz crashed.
- A renderer chosen in Settings that froze the last run goes back to Automatic.
- Fewer false "froze while presenting" reports after sleep, a locked screen or a hidden window.
- Pen input that fails inside the tablet driver no longer closes Eskiz.
- Pour Fill now applies when you lift a finger or pen.
- Magic wand drags no longer redo the work when the threshold has not changed.
- On a slow software graphics adapter, Eskiz waits longer before giving up on the GPU.
- Linux: the AppImage no longer carries a copy of a system library that needs a newer glibc than it promises, so it starts on older distributions.
- Linux: if the Wayland libraries are missing but an X11 display is available, Eskiz starts on X11 instead of stopping.
- Android: saving or exporting over an existing file keeps the old file if the copy does not finish, and a new file that fails to copy is removed.
- Android: a build with the `build` or `assemble` task now uses the release native library.
- The Windows download notes explain Smart App Control; the README steps for building and for macOS 15 are corrected.

### Known issues

- **Builds are not code-signed.** Your system will warn you the first time you open Eskiz.
  - macOS: Control-click Eskiz, choose Open, and confirm. If that does not work, open System Settings, Privacy & Security and choose Open Anyway, or run `xattr -dr com.apple.quarantine /Applications/Eskiz.app`.
  - Windows: extract the whole zip (Extract All) before running eskiz.exe. In the SmartScreen dialog, choose More info, then Run anyway.
  - Linux: make the AppImage executable with `chmod +x`.
  - Android: allow installing apps from your browser or file manager when asked.
- **Windows has been tested on GitHub's Windows machines, and on real PCs only by a few testers so far.** If the window ever stops responding, Eskiz starts in a safer graphics mode next time; you can also start it with `Eskiz (compatibility mode).bat` from the zip. Please [report it](https://github.com/evarnimtb-netizen/eskiz-app/issues/new?template=bug_report.yml) and attach `startup.log` from `%LOCALAPPDATA%\Eskiz\logs`.
- **Android is experimental.** Only arm64 devices running Android 10 or newer are supported. The APK is signed with a public debug key.
- **MP4 export is not included.** It needs the OpenH264 codec, which these builds do not bundle. GIF, APNG and PNG sequence export work. Developers can build with `--features mp4`.
- Some adjustment defaults and several 3D and Liquify details are approximations and may change.

[0.1.0]: https://github.com/evarnimtb-netizen/eskiz-app/releases/tag/v0.1.0
