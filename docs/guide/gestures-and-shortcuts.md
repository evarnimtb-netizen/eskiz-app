# Gestures and shortcuts

This chapter lists every touch gesture, key and canvas input. The tables are generated from the same source the app's Gestures & shortcuts sheet uses, so they match the app. The defaults are shown: a touch gesture that you switch off in Gesture controls is marked "(off)" in the app's sheet.

On Windows and Linux, read **Cmd** as **Ctrl**.

<!-- generated:shortcuts:begin: from crates/eskiz-app/src/user_guide.rs; do not edit -->

### Touch

| Gesture | What it does |
|---|---|
| One-finger drag | Paint with the current tool |
| Two-finger tap | Undo |
| Three-finger tap | Redo |
| Two-finger hold | Repeat undo |
| Three-finger scrub | Clear layer |
| Three-finger swipe | Copy menu |
| Four-finger tap | Full screen |
| Hold 0.4 s | Eyedropper |
| Hold 0.65 s at a stroke's end | Hold to Shape |
| Two fingers | Pan, zoom and rotate the view |

### Keyboard

| Keys | What it does |
|---|---|
| Cmd+Z | Undo |
| Cmd+Shift+Z | Redo |
| Cmd+0 | Fit canvas to window |
| Cmd+Backspace | Clear layer |
| Cmd+C, Cmd+X, Cmd+V | Copy, cut, paste |
| B, S, E | Brush, Smudge, Eraser |
| L | Layers panel |
| [  ] | Smaller, larger brush |
| F | Hide or show the interface |
| Esc | Close panel, cancel adjustment |
| Enter | Apply adjustment or transform |

### Canvas and pen

| Input | What it does |
|---|---|
| Space + drag | Pan |
| R + drag | Rotate the view |
| Wheel, Ctrl + wheel | Pan; with Ctrl, zoom |
| Alt + click, pen button | Pick a color |
| Hold still at a stroke's end | Hold to Shape |

<!-- generated:shortcuts:end -->

## Text editing

While a text layer is being edited, the keys go to the text instead of the tools: arrows, Shift with arrows to select, Home and End, Cmd+A, Backspace, Delete, Enter for a new line, and Esc to finish. See Text.

## Mouse, trackpad and pen

- **Space and drag** pans the view. **R and drag** rotates it.
- The **wheel** pans the view. **Ctrl and the wheel** zooms it.
- **Alt** (Option on macOS) and a click or drag pick a color (eyedropper).
- A pen's barrel button picks a color, as a hold of the Pick Color button does.
