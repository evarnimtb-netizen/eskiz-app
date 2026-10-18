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
| Cmd+Alt+Left, Cmd+Alt+Right | Rotate the canvas 90 degrees left, right |
| Cmd+Alt+0 | Reset canvas rotation |
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
| R + drag sideways | Rotate the view with a pen or mouse; it snaps at 0, 90, 180 and 270 degrees |
| Trackpad twist | Rotate the view (macOS trackpad rotate gesture) |
| Wheel, Ctrl + wheel | Pan; with Ctrl, zoom |
| Alt + click, pen button | Pick a color |
| Hold still at a stroke's end | Hold to Shape |

<!-- generated:shortcuts:end -->

## Text editing

While a text layer is being edited, the keys go to the text instead of the tools: arrows, Shift with arrows to select, Home and End, Cmd+A, Backspace, Delete, Enter for a new line, and Esc to finish. See Text.

## Mouse, trackpad and pen

- **Space and drag** pans the view. **R and drag sideways** rotates it (works with a pen, a mouse or a trackpad click-drag), and the view sticks at 0, 90, 180 and 270 degrees. On a Mac trackpad, **twist two fingers** to rotate; the view snaps the same way.
- **Rotate canvas 90° left / right** and **Reset rotation** are in the Canvas tab of the Actions menu, and on **Cmd+Alt+Left**, **Cmd+Alt+Right** and **Cmd+Alt+0**. A toast shows the angle ("Rotation 90°"). Rotating only turns the view: the picture is not changed, and painting, selection, transform and the brush cursor follow the turned canvas. **Cmd+0** fits the canvas and also turns it upright.
- The **wheel** pans the view. **Ctrl and the wheel** zooms it.
- **Alt** (Option on macOS) and a click or drag pick a color (eyedropper).
- A pen's barrel button picks a color, as a hold of the Pick Color button does.
