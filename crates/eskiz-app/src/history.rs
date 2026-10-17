//! Undo/redo stacks of [`Command`]s, and the [`PixelRect`] type commands use.
//!
//! The history keeps at most [`History::DEFAULT_MAX_STEPS`] steps and a byte budget of pixel
//! data, dropping the oldest steps first. The budget is [`History::DEFAULT_MAX_BYTES`] for small
//! canvases and grows with the canvas for large ones ([`History::bytes_cap_for`]). Commands
//! refer to layers by id, so deleting or reordering a layer never invalidates older steps.
//!
//! Pixel data in commands is shared copy-on-write tiles, so the byte budget counts every
//! distinct tile once, however many steps (or the live document) point at it.

use std::collections::{HashSet, VecDeque};

use eskiz_core::Depth;

use crate::command::Command;

/// An axis-aligned pixel rectangle inside a canvas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelRect {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}

impl PixelRect {
    /// Build from inclusive corners, clipped to a `width x height` canvas.
    /// `None` when nothing is left inside the canvas.
    pub fn from_inclusive(x0: i64, y0: i64, x1: i64, y1: i64, width: usize, height: usize) -> Option<Self> {
        let x0 = x0.max(0);
        let y0 = y0.max(0);
        let x1 = x1.min(width as i64 - 1);
        let y1 = y1.min(height as i64 - 1);
        if x0 > x1 || y0 > y1 {
            return None;
        }
        Some(Self {
            x: x0 as usize,
            y: y0 as usize,
            w: (x1 - x0 + 1) as usize,
            h: (y1 - y0 + 1) as usize,
        })
    }

    /// Smallest rectangle containing both.
    pub fn union(self, other: Self) -> Self {
        let x0 = self.x.min(other.x);
        let y0 = self.y.min(other.y);
        let x1 = (self.x + self.w).max(other.x + other.w);
        let y1 = (self.y + self.h).max(other.y + other.h);
        Self {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
        }
    }

    /// Bytes of RGBA8 pixel data in this rectangle.
    pub fn bytes(self) -> usize {
        self.w * self.h * 4
    }

    /// Copy this rectangle out of a full-width RGBA8 buffer.
    pub fn crop(self, pixels: &[u8], full_width: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.bytes());
        for row in self.y..self.y + self.h {
            let start = (row * full_width + self.x) * 4;
            out.extend_from_slice(&pixels[start..start + self.w * 4]);
        }
        out
    }

    /// Write `data` (as produced by [`PixelRect::crop`]) back into a full-width buffer.
    pub fn write(self, pixels: &mut [u8], full_width: usize, data: &[u8]) {
        let row_bytes = self.w * 4;
        for (r, row) in (self.y..self.y + self.h).enumerate() {
            let start = (row * full_width + self.x) * 4;
            pixels[start..start + row_bytes].copy_from_slice(&data[r * row_bytes..(r + 1) * row_bytes]);
        }
    }
}

/// Bounded undo/redo stack.
pub struct History {
    undo: VecDeque<Command>,
    redo: Vec<Command>,
    max_steps: usize,
    max_bytes: usize,
}

impl Default for History {
    fn default() -> Self {
        Self::with_defaults()
    }
}

impl History {
    pub const DEFAULT_MAX_STEPS: usize = 30;
    pub const DEFAULT_MAX_BYTES: usize = 256 * 1024 * 1024;

    pub fn new(max_steps: usize, max_bytes: usize) -> Self {
        Self {
            undo: VecDeque::new(),
            redo: Vec::new(),
            max_steps: max_steps.max(1),
            max_bytes,
        }
    }

    /// A history with the default caps.
    pub fn with_defaults() -> Self {
        Self::new(Self::DEFAULT_MAX_STEPS, Self::DEFAULT_MAX_BYTES)
    }

    /// Layers' worth of pixels the byte budget aims for on a large canvas.
    pub const LAYERS_OF_BUDGET: usize = 8;
    /// The share of physical memory the budget may take: one sixth.
    pub const MEMORY_DIVISOR: u64 = 6;
    /// Whatever the canvas and the machine, never more than this.
    pub const ABSOLUTE_MAX_BYTES: usize = 4 * 1024 * 1024 * 1024;
    /// The memory assumed when the machine's cannot be read.
    pub const ASSUMED_MEMORY: u64 = 8 * 1024 * 1024 * 1024;

    /// The undo byte budget for a `width x height` canvas of `depth` on a machine with
    /// `physical_memory` bytes of RAM (`None` when unknown).
    ///
    /// A step that touches a lot of a large canvas (a big soft brush, a filter) holds tens to
    /// hundreds of MiB, so a fixed budget leaves only a few steps there. The budget is
    /// [`History::LAYERS_OF_BUDGET`] full layers, but never more than a sixth of the RAM or
    /// [`History::ABSOLUTE_MAX_BYTES`], and never less than [`History::DEFAULT_MAX_BYTES`], so
    /// small canvases keep exactly the old budget.
    pub fn bytes_cap_for(width: usize, height: usize, depth: Depth, physical_memory: Option<u64>) -> usize {
        let layer = width.saturating_mul(height).saturating_mul(4).saturating_mul(depth.bytes_per_channel());
        let wanted = layer.saturating_mul(Self::LAYERS_OF_BUDGET);
        let ram = physical_memory.unwrap_or(Self::ASSUMED_MEMORY) / Self::MEMORY_DIVISOR;
        let ram = usize::try_from(ram).unwrap_or(usize::MAX);
        wanted.min(ram).clamp(Self::DEFAULT_MAX_BYTES, Self::ABSOLUTE_MAX_BYTES)
    }

    /// A history sized for this canvas on this machine: see [`History::bytes_cap_for`].
    pub fn for_canvas(width: usize, height: usize, depth: Depth) -> Self {
        Self::new(Self::DEFAULT_MAX_STEPS, Self::bytes_cap_for(width, height, depth, physical_memory()))
    }

    /// Change the byte budget (the canvas was resized or converted). Trims at once if needed.
    pub fn set_max_bytes(&mut self, max_bytes: usize) {
        if max_bytes != self.max_bytes {
            self.max_bytes = max_bytes;
            self.trim();
        }
    }

    /// The byte budget.
    pub fn max_bytes(&self) -> usize {
        self.max_bytes
    }

    /// Record a finished command. Clears the redo stack.
    pub fn push(&mut self, cmd: Command) {
        self.redo.clear();
        self.undo.push_back(cmd);
        self.trim();
    }

    /// Move the newest undo step to the redo stack and return it, for the caller to revert.
    pub fn undo(&mut self) -> Option<&Command> {
        let cmd = self.undo.pop_back()?;
        self.redo.push(cmd);
        self.redo.last()
    }

    /// Move the newest redo step back to the undo stack and return it, for the caller to re-apply.
    pub fn redo(&mut self) -> Option<&Command> {
        let cmd = self.redo.pop()?;
        self.undo.push_back(cmd);
        self.undo.back()
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// The step that undo would revert next.
    pub fn next_undo(&self) -> Option<&Command> {
        self.undo.back()
    }

    /// The step that redo would re-apply next.
    pub fn next_redo(&self) -> Option<&Command> {
        self.redo.last()
    }

    /// Mutable access to the newest undo step (to extend a slider drag in place).
    pub fn last_undo_mut(&mut self) -> Option<&mut Command> {
        self.undo.back_mut()
    }

    /// Number of undo steps available.
    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    /// Total pixel bytes held by both stacks: each distinct tile counts once, even when
    /// several steps share it.
    pub fn bytes(&self) -> usize {
        let mut seen = HashSet::new();
        self.undo.iter().chain(self.redo.iter()).map(|c| c.account(&mut seen)).sum()
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    fn trim(&mut self) {
        while self.undo.len() > self.max_steps || (self.bytes() > self.max_bytes && self.undo.len() > 1) {
            self.undo.pop_front();
        }
    }
}

/// Physical memory of this machine in bytes, read once (the smaller of the RAM and, on Linux,
/// the memory limit of the control group). `None` where it cannot be read.
pub fn physical_memory() -> Option<u64> {
    static MEM: std::sync::OnceLock<Option<u64>> = std::sync::OnceLock::new();
    *MEM.get_or_init(read_physical_memory)
}

/// `MemTotal` of a `/proc/meminfo` text, in bytes (the file counts KiB).
#[cfg(any(target_os = "linux", target_os = "android", test))]
pub(crate) fn parse_meminfo_total(text: &str) -> Option<u64> {
    let line = text.lines().find(|l| l.starts_with("MemTotal:"))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    kib.checked_mul(1024)
}

/// Linux: `/proc/meminfo`, capped by the control-group limit when there is one.
#[cfg(target_os = "linux")]
fn read_physical_memory() -> Option<u64> {
    let total = std::fs::read_to_string("/proc/meminfo").ok().and_then(|s| parse_meminfo_total(&s));
    let limit = ["/sys/fs/cgroup/memory.max", "/sys/fs/cgroup/memory/memory.limit_in_bytes"]
        .iter()
        .find_map(|p| std::fs::read_to_string(p).ok()?.trim().parse::<u64>().ok());
    match (total, limit) {
        (Some(t), Some(l)) => Some(t.min(l)),
        (t, l) => t.or(l),
    }
}

/// Android: `/proc/meminfo` only (apps cannot read the control-group files of other groups, and
/// the kernel's `MemTotal` is the phone's RAM).
#[cfg(target_os = "android")]
fn read_physical_memory() -> Option<u64> {
    std::fs::read_to_string("/proc/meminfo").ok().and_then(|s| parse_meminfo_total(&s))
}

#[cfg(target_os = "macos")]
fn read_physical_memory() -> Option<u64> {
    let out = std::process::Command::new("sysctl").args(["-n", "hw.memsize"]).output().ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos")))]
fn read_physical_memory() -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::Change;
    use eskiz_core::canvas::{Layer, TILE_BYTES};

    /// A one-change command holding one tile before and one after (`fill` marks it).
    fn entry(fill: u8, _size: usize) -> Command {
        let rect = PixelRect { x: 0, y: 0, w: 4, h: 4 };
        let mut before = Layer::new(4, 4);
        before.set_pixel(0, 0, [fill, 0, 0, 255]);
        let mut after = before.clone();
        after.set_pixel(1, 1, [fill, 1, 1, 255]);
        Command::single("Stroke", Change::tiles_between(1, &before, &after, rect))
    }

    fn first_fill(c: &Command) -> u8 {
        match &c.changes[0] {
            Change::Tiles { before, .. } => before[0].tile.as_ref().unwrap().data()[0],
            _ => unreachable!(),
        }
    }

    #[test]
    fn from_inclusive_clips_to_canvas() {
        assert_eq!(
            PixelRect::from_inclusive(-5, -5, 3, 2, 10, 10),
            Some(PixelRect { x: 0, y: 0, w: 4, h: 3 })
        );
        assert_eq!(
            PixelRect::from_inclusive(8, 8, 20, 20, 10, 10),
            Some(PixelRect { x: 8, y: 8, w: 2, h: 2 })
        );
        assert_eq!(PixelRect::from_inclusive(10, 0, 12, 3, 10, 10), None);
        assert_eq!(PixelRect::from_inclusive(-9, 0, -1, 3, 10, 10), None);
    }

    #[test]
    fn union_covers_both() {
        let a = PixelRect { x: 2, y: 2, w: 3, h: 3 };
        let b = PixelRect { x: 6, y: 0, w: 2, h: 1 };
        assert_eq!(a.union(b), PixelRect { x: 2, y: 0, w: 6, h: 5 });
        assert_eq!(a.union(a), a);
    }

    #[test]
    fn crop_then_write_round_trips() {
        let width = 6;
        let height = 4;
        let original: Vec<u8> = (0..width * height * 4).map(|v| v as u8).collect();
        let rect = PixelRect { x: 1, y: 1, w: 3, h: 2 };
        let data = rect.crop(&original, width);
        assert_eq!(data.len(), rect.bytes());
        // First cropped pixel is (1,1).
        assert_eq!(&data[0..4], &original[(width + 1) * 4..(width + 1) * 4 + 4]);

        let mut target = vec![0u8; original.len()];
        rect.write(&mut target, width, &data);
        assert_eq!(rect.crop(&target, width), data);
        // Pixels outside the rectangle stay zero.
        assert_eq!(target[0], 0);
    }

    #[test]
    fn undo_and_redo_move_commands_between_stacks() {
        let mut h = History::with_defaults();
        h.push(entry(10, 2));
        h.push(entry(20, 2));
        assert_eq!(h.undo_len(), 2);
        assert_eq!(h.undo().map(first_fill), Some(20));
        assert!(h.can_redo());
        assert_eq!(h.redo().map(first_fill), Some(20));
        assert!(!h.can_redo());
        assert!(h.can_undo());
    }

    #[test]
    fn new_command_clears_redo() {
        let mut h = History::with_defaults();
        h.push(entry(10, 2));
        h.undo();
        assert!(h.can_redo());
        h.push(entry(30, 2));
        assert!(!h.can_redo());
    }

    #[test]
    fn step_cap_drops_oldest() {
        let mut h = History::with_defaults();
        for i in 0..(History::DEFAULT_MAX_STEPS + 5) {
            h.push(entry(i as u8, 1));
        }
        assert_eq!(h.undo_len(), History::DEFAULT_MAX_STEPS);
        // The oldest surviving step is the 6th one pushed (fill 5).
        assert_eq!(h.undo.front().map(first_fill), Some(5));
    }

    #[test]
    fn byte_cap_drops_oldest_but_keeps_newest() {
        // Each entry holds two distinct tiles, so a budget of five tiles keeps two entries.
        let mut h = History::new(30, 5 * TILE_BYTES);
        for i in 0..6 {
            h.push(entry(i as u8, 2));
        }
        assert!(h.bytes() <= 5 * TILE_BYTES);
        assert_eq!(h.undo_len(), 2);
        // A single entry larger than the budget is still kept.
        let mut tiny = History::new(30, 1);
        tiny.push(entry(7, 4));
        assert_eq!(tiny.undo_len(), 1);
    }

    #[test]
    fn shared_tiles_are_counted_once() {
        // Two strokes on one tile: the second stroke's "before" is the first one's "after".
        let rect = PixelRect { x: 0, y: 0, w: 4, h: 4 };
        let a = Layer::new(4, 4);
        let mut b = a.clone();
        b.set_pixel(0, 0, [1, 1, 1, 255]);
        let mut c = b.clone();
        c.set_pixel(1, 1, [2, 2, 2, 255]);
        let mut h = History::with_defaults();
        h.push(Command::single("Stroke", Change::tiles_between(1, &a, &b, rect)));
        assert_eq!(h.bytes(), TILE_BYTES, "the empty 'before' tile costs nothing");
        h.push(Command::single("Stroke", Change::tiles_between(1, &b, &c, rect)));
        assert_eq!(h.bytes(), 2 * TILE_BYTES, "b's tile is held by both steps but counted once");
        let one: usize = h.undo.iter().map(Command::bytes).sum();
        assert_eq!(one, 3 * TILE_BYTES, "per-command totals double count it");
        h.undo();
        assert_eq!(h.bytes(), 2 * TILE_BYTES, "redo entries count too");
    }

    #[test]
    fn byte_cap_scales_with_the_canvas_and_the_machine() {
        const GIB: u64 = 1024 * 1024 * 1024;
        let cap = |w, h, d, ram| History::bytes_cap_for(w, h, d, ram);
        // Small and medium canvases keep the old budget exactly.
        for (w, h) in [(1, 1), (1024, 1024), (2048, 2048), (3000, 2000)] {
            assert_eq!(cap(w, h, Depth::U8, Some(16 * GIB)), History::DEFAULT_MAX_BYTES, "{w}x{h}");
        }
        // Large canvases get eight layers' worth, bounded by the machine.
        assert_eq!(cap(4096, 4096, Depth::U8, Some(64 * GIB)), 8 * 64 * 1024 * 1024);
        assert_eq!(cap(8000, 8000, Depth::U8, Some(64 * GIB)), 8 * 256_000_000);
        assert_eq!(cap(8000, 8000, Depth::U16, Some(64 * GIB)), 8 * 512_000_000);
        assert_eq!(cap(8000, 8000, Depth::U8, Some(8 * GIB)), (8 * GIB / History::MEMORY_DIVISOR) as usize);
        assert_eq!(cap(8000, 8000, Depth::U8, Some(GIB)), History::DEFAULT_MAX_BYTES, "never below the floor");
        assert_eq!(cap(16384, 16384, Depth::U16, Some(1024 * GIB)), History::ABSOLUTE_MAX_BYTES);
        assert_eq!(cap(8000, 8000, Depth::U8, None), (History::ASSUMED_MEMORY / History::MEMORY_DIVISOR) as usize);
        // Monotonic in the canvas size.
        let mut last = 0;
        for side in [512, 1024, 2048, 4096, 6000, 8000, 16000] {
            let c = cap(side, side, Depth::U8, Some(32 * GIB));
            assert!(c >= last);
            last = c;
        }
    }

    #[test]
    fn raising_the_budget_keeps_steps_and_lowering_it_trims() {
        let mut h = History::new(30, 5 * TILE_BYTES);
        for i in 0..6 {
            h.push(entry(i as u8, 2));
        }
        assert_eq!(h.undo_len(), 2);
        h.set_max_bytes(40 * TILE_BYTES);
        for i in 6..14 {
            h.push(entry(i as u8, 2));
        }
        assert_eq!(h.undo_len(), 10, "a larger budget keeps more steps");
        h.set_max_bytes(5 * TILE_BYTES);
        assert_eq!(h.undo_len(), 2);
        assert_eq!(h.undo.back().map(first_fill), Some(13), "the newest steps survive");
    }

    #[test]
    fn meminfo_text_gives_total_ram_in_bytes() {
        // A trimmed copy of what an Android 14 phone reports.
        let phone = "MemTotal:        7864320 kB\nMemFree:          812344 kB\nMemAvailable:    3011720 kB\n";
        assert_eq!(parse_meminfo_total(phone), Some(7_864_320 * 1024));
        assert_eq!(parse_meminfo_total("MemFree: 1 kB\n"), None);
        assert_eq!(parse_meminfo_total("MemTotal: lots kB\n"), None);
        assert_eq!(parse_meminfo_total(""), None);
    }

    #[test]
    fn android_ram_sets_the_undo_cap() {
        // 8 GiB phone, huge canvas: a sixth of RAM. 3 GiB phone: a sixth of that, floor applies.
        let big = History::bytes_cap_for(8000, 8000, Depth::default(), parse_meminfo_total("MemTotal: 7864320 kB"));
        assert_eq!(big, (7_864_320u64 * 1024 / History::MEMORY_DIVISOR) as usize);
        let small_phone = History::bytes_cap_for(8000, 8000, Depth::default(), parse_meminfo_total("MemTotal: 3145728 kB"));
        assert_eq!(small_phone, (3_145_728u64 * 1024 / History::MEMORY_DIVISOR) as usize);
        assert!(small_phone < big);
        // An unreadable file falls back to the assumed memory, never to zero.
        let none = History::bytes_cap_for(8000, 8000, Depth::default(), parse_meminfo_total("garbage"));
        assert!(none >= History::DEFAULT_MAX_BYTES);
    }

    #[test]
    fn physical_memory_is_read_on_this_machine() {
        if cfg!(any(target_os = "linux", target_os = "macos")) {
            assert!(physical_memory().is_some_and(|m| m >= 64 * 1024 * 1024));
        }
    }
}
