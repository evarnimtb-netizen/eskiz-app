# Changelog

All notable changes to Eskiz are listed here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses [Semantic Versioning](https://semver.org/).

## [0.1.0] - Unreleased

First public preview of Eskiz, a free, open source painting app for macOS, Windows, Linux and (experimentally) Android.

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
- User guide: the welcome tour is skipped for data folders that already hold artworks; Help > Welcome tour shows it.

### Known issues

- **Builds are not code-signed.** Your system will warn you the first time you open Eskiz.
  - macOS: Control-click Eskiz, choose Open, and confirm. If that does not work, open System Settings, Privacy & Security and choose Open Anyway, or run `xattr -dr com.apple.quarantine /Applications/Eskiz.app`.
  - Windows: in the SmartScreen dialog, choose More info, then Run anyway.
  - Linux: make the AppImage executable with `chmod +x`.
  - Android: allow installing apps from your browser or file manager when asked.
- **Windows has not yet been tested on real hardware.** The Windows build passes the automated tests and self-test on GitHub's Windows runners only.
- **Android is experimental.** There is no system file picker, share sheet or clipboard integration yet, and only arm64 devices running Android 10 or newer are supported. The APK is signed with a public debug key.
- **MP4 export is not included.** It needs the OpenH264 codec, which these builds do not bundle. GIF, APNG and PNG sequence export work. Developers can build with `--features mp4`.
- Some adjustment defaults and several 3D and Liquify details are approximations and may change.

[0.1.0]: https://github.com/evarnimtb-netizen/eskiz-app/releases/tag/v0.1.0
