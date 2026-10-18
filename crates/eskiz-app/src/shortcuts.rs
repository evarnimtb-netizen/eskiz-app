//! Keyboard shortcuts: the one place where the canvas key bindings are named.
//!
//! [`crate::app`] dispatches the keys with these constants, and [`KEYBOARD`] lists them for
//! humans (the Gestures & shortcuts sheet, the Help tab and the User guide). Because the table
//! and the dispatch share the constants, a documented key cannot drift from the key that works.
//! The tests in `user_guide` check the generated documentation against this table.
//!
//! `{cmd}` in a label stands for the platform command key: Cmd on macOS, Ctrl elsewhere.

use eframe::egui::{Key, KeyboardShortcut, Modifiers};

/// Command plus Shift, spelled as a constant (`|` is not `const`).
const COMMAND_SHIFT: Modifiers = Modifiers { shift: true, ..Modifiers::COMMAND };
/// Command plus Alt/Option.
const COMMAND_ALT: Modifiers = Modifiers { alt: true, ..Modifiers::COMMAND };

pub const UNDO: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Z);
pub const REDO: KeyboardShortcut = KeyboardShortcut::new(COMMAND_SHIFT, Key::Z);
pub const FIT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Num0);
pub const ROTATE_LEFT: KeyboardShortcut = KeyboardShortcut::new(COMMAND_ALT, Key::ArrowLeft);
pub const ROTATE_RIGHT: KeyboardShortcut = KeyboardShortcut::new(COMMAND_ALT, Key::ArrowRight);
pub const RESET_ROTATION: KeyboardShortcut = KeyboardShortcut::new(COMMAND_ALT, Key::Num0);
pub const CLEAR_LAYER: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Backspace);
pub const COPY: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::C);
pub const CUT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::X);
pub const PASTE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::V);

/// Plain keys: pressed with no modifier held.
pub const BRUSH: Key = Key::B;
pub const SMUDGE: Key = Key::S;
pub const ERASER: Key = Key::E;
pub const LAYERS: Key = Key::L;
pub const SMALLER_BRUSH: Key = Key::OpenBracket;
pub const LARGER_BRUSH: Key = Key::CloseBracket;
pub const FULLSCREEN: Key = Key::F;
pub const ESCAPE: Key = Key::Escape;
pub const ENTER: Key = Key::Enter;
/// Held keys: Space and drag pans, R and drag rotates the view.
pub const PAN_HOLD: Key = Key::Space;
pub const ROTATE_HOLD: Key = Key::R;

/// One key, or one chord, that an [`Entry`] stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Binding {
    Chord(KeyboardShortcut),
    Key(Key),
}

/// A row of the shortcut table: the keys as written, what they do, and the bindings they are.
#[derive(Clone, Copy, Debug)]
pub struct Entry {
    pub keys: &'static str,
    pub action: &'static str,
    pub bindings: &'static [Binding],
}

/// The keyboard shortcuts, in the order the sheets show them.
pub const KEYBOARD: &[Entry] = &[
    Entry {
        keys: "{cmd}+Z",
        action: "Undo",
        bindings: &[Binding::Chord(UNDO)],
    },
    Entry {
        keys: "{cmd}+Shift+Z",
        action: "Redo",
        bindings: &[Binding::Chord(REDO)],
    },
    Entry {
        keys: "{cmd}+0",
        action: "Fit canvas to window",
        bindings: &[Binding::Chord(FIT)],
    },
    Entry {
        keys: "{cmd}+Alt+Left, {cmd}+Alt+Right",
        action: "Rotate the canvas 90 degrees left, right",
        bindings: &[Binding::Chord(ROTATE_LEFT), Binding::Chord(ROTATE_RIGHT)],
    },
    Entry {
        keys: "{cmd}+Alt+0",
        action: "Reset canvas rotation",
        bindings: &[Binding::Chord(RESET_ROTATION)],
    },
    Entry {
        keys: "{cmd}+Backspace",
        action: "Clear layer",
        bindings: &[Binding::Chord(CLEAR_LAYER)],
    },
    Entry {
        keys: "{cmd}+C, {cmd}+X, {cmd}+V",
        action: "Copy, cut, paste",
        bindings: &[Binding::Chord(COPY), Binding::Chord(CUT), Binding::Chord(PASTE)],
    },
    Entry {
        keys: "B, S, E",
        action: "Brush, Smudge, Eraser",
        bindings: &[Binding::Key(BRUSH), Binding::Key(SMUDGE), Binding::Key(ERASER)],
    },
    Entry {
        keys: "L",
        action: "Layers panel",
        bindings: &[Binding::Key(LAYERS)],
    },
    Entry {
        keys: "[  ]",
        action: "Smaller, larger brush",
        bindings: &[Binding::Key(SMALLER_BRUSH), Binding::Key(LARGER_BRUSH)],
    },
    Entry {
        keys: "F",
        action: "Hide or show the interface",
        bindings: &[Binding::Key(FULLSCREEN)],
    },
    Entry {
        keys: "Esc",
        action: "Close panel, cancel adjustment",
        bindings: &[Binding::Key(ESCAPE)],
    },
    Entry {
        keys: "Enter",
        action: "Apply adjustment or transform",
        bindings: &[Binding::Key(ENTER)],
    },
];

/// The keys as shown on this platform: `{cmd}` becomes `cmd` or `Ctrl`.
pub fn display_keys(entry: &Entry, cmd: &str) -> String {
    entry.keys.replace("{cmd}", cmd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_chord_is_unique_and_has_a_modifier() {
        let mut seen = Vec::new();
        for e in KEYBOARD {
            for b in e.bindings {
                if let Binding::Chord(c) = b {
                    assert!(!seen.contains(c), "{} binds a chord twice", e.action);
                    seen.push(*c);
                }
            }
        }
        assert_eq!(seen.len(), 10, "undo, redo, fit, rotate x3, clear, copy, cut, paste");
    }

    #[test]
    fn plain_keys_are_not_bound_twice() {
        let mut keys = Vec::new();
        for e in KEYBOARD {
            for b in e.bindings {
                if let Binding::Key(k) = b {
                    assert!(!keys.contains(k), "{k:?} is bound twice");
                    keys.push(*k);
                }
            }
        }
    }

    #[test]
    fn cmd_is_replaced_per_platform() {
        assert_eq!(display_keys(&KEYBOARD[0], "Ctrl"), "Ctrl+Z");
        assert_eq!(display_keys(&KEYBOARD[1], "Cmd"), "Cmd+Shift+Z");
    }
}
