//! The safe area: the strips of the window the system bars and display cutouts cover.
//!
//! On Android the app is edge-to-edge (targetSdk 36), so the status bar, navigation bar and
//! cutouts sit on top of the window. The Java shim reads the insets (`WindowInsets`), passes
//! them over JNI in physical pixels ([`set_pixels`]), and the app converts them to points
//! ([`SafeArea::from_pixels`]) and hands them to egui as `safe_area_insets`, so every layout
//! that works from `content_rect()` (top bar, sidebar, popovers, sheets, gallery) stays clear
//! of the bars. The strips themselves are painted with the bar colour. On desktop the safe
//! area is zero and nothing changes.

use std::sync::atomic::{AtomicI32, Ordering};

use eframe::egui::{self, epaint::MarginF32, SafeAreaInsets};

/// Insets in points.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SafeArea {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl SafeArea {
    pub const ZERO: SafeArea = SafeArea { left: 0.0, top: 0.0, right: 0.0, bottom: 0.0 };

    /// Insets in physical pixels `[left, top, right, bottom]` to points. Negative values and a
    /// zero scale count as zero.
    pub fn from_pixels(px: [i32; 4], pixels_per_point: f32) -> Self {
        let s = if pixels_per_point > 0.0 { pixels_per_point } else { 1.0 };
        let p = |v: i32| v.max(0) as f32 / s;
        Self { left: p(px[0]), top: p(px[1]), right: p(px[2]), bottom: p(px[3]) }
    }

    pub fn is_zero(&self) -> bool {
        *self == Self::ZERO
    }

    pub fn insets(&self) -> SafeAreaInsets {
        SafeAreaInsets(MarginF32 { left: self.left, right: self.right, top: self.top, bottom: self.bottom })
    }

    /// The strips of `screen` the bars cover: top, bottom, left, right (empty ones skipped).
    pub fn strips(&self, screen: egui::Rect) -> Vec<egui::Rect> {
        let mut v = Vec::new();
        let (l, r, t, b) = (screen.left(), screen.right(), screen.top(), screen.bottom());
        if self.top > 0.0 {
            v.push(egui::Rect::from_min_max(egui::pos2(l, t), egui::pos2(r, t + self.top)));
        }
        if self.bottom > 0.0 {
            v.push(egui::Rect::from_min_max(egui::pos2(l, b - self.bottom), egui::pos2(r, b)));
        }
        if self.left > 0.0 {
            v.push(egui::Rect::from_min_max(egui::pos2(l, t + self.top), egui::pos2(l + self.left, b - self.bottom)));
        }
        if self.right > 0.0 {
            v.push(egui::Rect::from_min_max(egui::pos2(r - self.right, t + self.top), egui::pos2(r, b - self.bottom)));
        }
        v
    }
}

static PIXELS: [AtomicI32; 4] = [AtomicI32::new(0), AtomicI32::new(0), AtomicI32::new(0), AtomicI32::new(0)];

/// Store the insets the Java shim reports (physical pixels).
pub fn set_pixels(left: i32, top: i32, right: i32, bottom: i32) {
    for (a, v) in PIXELS.iter().zip([left, top, right, bottom]) {
        a.store(v, Ordering::SeqCst);
    }
}

/// The last insets stored, `[left, top, right, bottom]` in pixels.
pub fn pixels() -> [i32; 4] {
    [0, 1, 2, 3].map(|i| PIXELS[i].load(Ordering::SeqCst))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixels_become_points() {
        let s = SafeArea::from_pixels([0, 96, 0, 144], 2.0);
        assert_eq!((s.top, s.bottom, s.left, s.right), (48.0, 72.0, 0.0, 0.0));
        assert!(SafeArea::from_pixels([-5, 0, 0, 0], 0.0).is_zero());
    }

    #[test]
    fn strips_do_not_overlap() {
        let s = SafeArea { left: 10.0, top: 20.0, right: 10.0, bottom: 30.0 };
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(200.0, 300.0));
        let strips = s.strips(screen);
        assert_eq!(strips.len(), 4);
        let area: f32 = strips.iter().map(|r| r.area()).sum();
        assert_eq!(area, 200.0 * 300.0 - 180.0 * 250.0);
    }
}
