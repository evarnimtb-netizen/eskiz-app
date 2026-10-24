//! The system clipboard for images (A4).
//!
//! Copy, Cut and Copy all keep Eskiz's own [`Clip`] and, where the platform has a system
//! clipboard for images, also put the pixels there as a PNG. Paste uses an image another app
//! copied when that is what the system clipboard holds now. The seam is [`SystemClipboard`]:
//! desktop uses [`NoSystemClipboard`] (nothing changes there), tests use [`MockSystemClipboard`]
//! and Android uses the bridge in `android_bridge.rs`.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Mutex;

use eskiz_select::clipboard::Clip;
use eskiz_select::Image;

use crate::app::EskizApp;

/// What the system clipboard holds right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipState {
    /// Nothing, or something Eskiz cannot use.
    None,
    /// Text only.
    TextOnly,
    /// The image Eskiz itself put there, with the serial of the copy.
    Ours(u64),
    /// An image another app copied.
    ForeignImage,
}

pub trait SystemClipboard: Send + Sync {
    /// Where the PNG of a copy is written, or `None` when there is no system clipboard for images.
    fn clip_dir(&self) -> Option<PathBuf> {
        None
    }
    /// Put the PNG at `png` (written by [`write_clip_png`]) on the system clipboard as copy `serial`.
    fn put_image(&self, _png: &Path, _serial: u64) {}
    /// What the clipboard holds now. Quick.
    fn state(&self) -> ClipState {
        ClipState::None
    }
    /// Copy the image another app put on the clipboard into the cache. Never blocks.
    fn read_image(&self) -> Receiver<Result<PathBuf, String>> {
        let (tx, rx) = channel();
        let _ = tx.send(Err("There is no image on the clipboard.".to_owned()));
        rx
    }
}

/// Desktop and tests that do not script a clipboard: Copy and Paste stay inside Eskiz.
pub struct NoSystemClipboard;

impl SystemClipboard for NoSystemClipboard {}

/// A scripted clipboard for the host tests.
#[derive(Default)]
pub struct MockSystemClipboard {
    dir: Mutex<Option<PathBuf>>,
    state: Mutex<Option<ClipState>>,
    puts: Mutex<Vec<(PathBuf, u64)>>,
    foreign: Mutex<Option<Result<PathBuf, String>>>,
}

impl MockSystemClipboard {
    pub fn new() -> Self {
        Self::default()
    }

    /// A mock that writes copies into `dir`.
    pub fn in_dir(dir: PathBuf) -> Self {
        let m = Self::new();
        *m.dir.lock().expect("mock clip dir") = Some(dir);
        m
    }

    pub fn set_state(&self, state: ClipState) {
        *self.state.lock().expect("mock clip state") = Some(state);
    }

    /// Another app copies the image at `png`: the state becomes `ForeignImage`.
    pub fn copy_foreign(&self, png: PathBuf) {
        *self.foreign.lock().expect("mock foreign") = Some(Ok(png));
        self.set_state(ClipState::ForeignImage);
    }

    /// Another app copies an image that cannot be read.
    pub fn copy_broken(&self, error: &str) {
        *self.foreign.lock().expect("mock foreign") = Some(Err(error.to_owned()));
        self.set_state(ClipState::ForeignImage);
    }

    /// Every `put_image` call: the file and the serial.
    pub fn puts(&self) -> Vec<(PathBuf, u64)> {
        self.puts.lock().expect("mock puts").clone()
    }
}

impl SystemClipboard for MockSystemClipboard {
    fn clip_dir(&self) -> Option<PathBuf> {
        self.dir.lock().expect("mock clip dir").clone()
    }

    fn put_image(&self, png: &Path, serial: u64) {
        self.puts.lock().expect("mock puts").push((png.to_path_buf(), serial));
        self.set_state(ClipState::Ours(serial));
    }

    fn state(&self) -> ClipState {
        self.state.lock().expect("mock clip state").unwrap_or(ClipState::None)
    }

    fn read_image(&self) -> Receiver<Result<PathBuf, String>> {
        let (tx, rx) = channel();
        let answer = self.foreign.lock().expect("mock foreign").clone().unwrap_or_else(|| Err("There is no image on the clipboard.".to_owned()));
        let _ = tx.send(answer);
        rx
    }
}

/// The clipboard the app starts with: the Android bridge there, else [`NoSystemClipboard`]. Unit
/// tests always get [`NoSystemClipboard`].
pub fn default_system_clipboard() -> std::sync::Arc<dyn SystemClipboard> {
    #[cfg(all(target_os = "android", not(test)))]
    {
        std::sync::Arc::new(crate::android_bridge::AndroidClipboard::new())
    }
    #[cfg(not(all(target_os = "android", not(test))))]
    {
        std::sync::Arc::new(NoSystemClipboard)
    }
}

// ------------------------------------------------------------------------------------------------
// Pixels

/// Encode a clip as a straight-alpha RGBA8 PNG at its own size.
pub fn clip_to_png(clip: &Clip) -> Result<Vec<u8>, String> {
    let (w, h) = (clip.image.width, clip.image.height);
    if w == 0 || h == 0 {
        return Err("The copy is empty.".to_owned());
    }
    let mut bytes = Vec::with_capacity(w * h * 4);
    for p in &clip.image.pixels {
        for c in p {
            bytes.push((c.clamp(0.0, 1.0) * 255.0).round() as u8);
        }
    }
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, w as u32, h as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::Fast);
    let mut writer = enc.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(&bytes).map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())?;
    Ok(out)
}

/// Decode a straight-alpha RGBA8 PNG into a clip image.
pub fn png_to_image(bytes: &[u8]) -> Result<Image, String> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or("The picture is too large.")?];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return Err("Unsupported picture format.".to_owned());
    }
    let (w, h) = (info.width as usize, info.height as usize);
    Ok(Image::from_fn(w, h, |x, y| {
        let i = (y * w + x) * 4;
        [f32::from(buf[i]) / 255.0, f32::from(buf[i + 1]) / 255.0, f32::from(buf[i + 2]) / 255.0, f32::from(buf[i + 3]) / 255.0]
    }))
}

/// `clip-<serial>.png`, the file for copy `serial`.
pub fn clip_file_name(serial: u64) -> String {
    format!("clip-{serial}.png")
}

/// Write the PNG of copy `serial` into `dir` and remove older clip files. Returns the path.
pub fn write_clip_png(dir: &Path, serial: u64, clip: &Clip) -> Result<PathBuf, String> {
    let png = clip_to_png(clip)?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let path = dir.join(clip_file_name(serial));
    let tmp = dir.join(format!(".clip-{serial}.tmp"));
    std::fs::write(&tmp, png).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            // Only older copies go: two quick copies encode on two threads, and the older one
            // may finish last.
            let older = name
                .strip_prefix("clip-")
                .and_then(|n| n.strip_suffix(".png"))
                .and_then(|n| n.parse::<u64>().ok())
                .is_some_and(|n| n < serial);
            if older {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    Ok(path)
}

// ------------------------------------------------------------------------------------------------
// The app side

/// How long a foreign paste may wait for the other app's picture.
const PASTE_TIMEOUT_SECS: f64 = 20.0;

/// Copies in flight and the paste of a foreign image.
#[derive(Default)]
pub struct ClipJobs {
    /// Serial of the latest copy.
    pub serial: u64,
    /// The encode of a copy: `(path, serial)` when written.
    put: Option<Receiver<Result<(PathBuf, u64), String>>>,
    /// The cache copy of a foreign image.
    read: Option<Receiver<Result<PathBuf, String>>>,
    /// The decoded, fitted pixels of a foreign image (premultiplied canvas-sized RGBA8).
    decode: Option<Receiver<Result<Vec<u8>, String>>>,
    /// The drawing the foreign paste was started for (`Session::document_generation`).
    doc: u64,
    /// When the foreign paste started (app time): a provider that never answers is given up.
    since: f64,
    memo: std::cell::Cell<Option<(f64, ClipState)>>,
}

impl ClipJobs {
    pub fn busy(&self) -> bool {
        self.put.is_some() || self.read.is_some() || self.decode.is_some()
    }
}

/// How long a `state()` answer is reused while the Paste button is drawn.
const STATE_MEMO_SECS: f64 = 0.5;

impl EskizApp {
    /// Also put the clip just made on the system clipboard (where there is one).
    pub(crate) fn clip_to_system(&mut self) {
        let Some(dir) = self.sysclip.clip_dir() else { return };
        let Some(clip) = self.sel.clip.clone() else { return };
        self.clips.serial += 1;
        let serial = self.clips.serial;
        let (tx, rx) = channel();
        let spawned = std::thread::Builder::new().name("eskiz-clip".into()).spawn(move || {
            let _ = tx.send(write_clip_png(&dir, serial, &clip).map(|p| (p, serial)));
        });
        if spawned.is_ok() {
            self.clips.put = Some(rx);
        }
    }

    /// The system clipboard's state, reused for half a second while a panel asks every frame.
    pub(crate) fn clip_state_cached(&self) -> ClipState {
        if let Some((at, state)) = self.clips.memo.get() {
            if self.now - at < STATE_MEMO_SECS && self.now >= at {
                return state;
            }
        }
        let state = self.sysclip.state();
        self.clips.memo.set(Some((self.now, state)));
        state
    }

    /// Is there an image to paste, from Eskiz or from another app?
    pub(crate) fn can_paste(&self) -> bool {
        self.sel.clip.is_some() || self.clip_state_cached() == ClipState::ForeignImage
    }

    /// Paste an image another app copied. Returns false when the system clipboard holds no foreign
    /// image (the caller then pastes Eskiz's own clip).
    pub(crate) fn start_foreign_paste(&mut self) -> bool {
        self.clips.memo.set(None);
        if self.sysclip.state() != ClipState::ForeignImage {
            return false;
        }
        if self.clips.read.is_some() || self.clips.decode.is_some() {
            self.show_toast("A paste is already running.", 1.5);
            return true;
        }
        self.clips.read = Some(self.sysclip.read_image());
        self.clips.doc = self.session.document_generation();
        self.clips.since = self.now;
        self.show_toast("Pasting...", 10.0);
        true
    }

    /// Advance the clipboard jobs. Called once per frame.
    pub(crate) fn poll_clip_jobs(&mut self, ctx: &eframe::egui::Context) {
        if let Some(rx) = self.clips.put.as_ref() {
            match rx.try_recv() {
                Ok(Ok((path, serial))) => {
                    self.clips.put = None;
                    // Only the latest copy goes to the system clipboard.
                    if serial == self.clips.serial {
                        self.sysclip.put_image(&path, serial);
                        self.clips.memo.set(None);
                    }
                }
                Ok(Err(e)) => {
                    self.clips.put = None;
                    self.show_toast(format!("Copied inside Eskiz only: {e}"), 3.0);
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => self.clips.put = None,
            }
        }
        if let Some(rx) = self.clips.read.as_ref() {
            match rx.try_recv() {
                Ok(Ok(path)) => {
                    self.clips.read = None;
                    let (cw, ch) = (self.session.doc.width, self.session.doc.height);
                    let (tx, rx) = channel();
                    let repaint = ctx.clone();
                    let spawned = std::thread::Builder::new().name("eskiz-clip-decode".into()).spawn(move || {
                        let result = crate::import::decode_image(&path).map(|(w, h, rgba)| {
                            crate::saf::log_line(&format!("clip: foreign {w}x{h}"));
                            crate::import::fit_to_canvas(&rgba, w, h, cw, ch)
                        });
                        let _ = std::fs::remove_file(&path);
                        let _ = tx.send(result);
                        repaint.request_repaint();
                    });
                    if spawned.is_ok() {
                        self.clips.decode = Some(rx);
                    }
                }
                Ok(Err(e)) => {
                    self.clips.read = None;
                    self.show_toast(format!("Could not paste: {e}"), 4.0);
                }
                Err(TryRecvError::Empty) if self.now - self.clips.since > PASTE_TIMEOUT_SECS => {
                    // A content provider waiting for the network (or never answering): give up
                    // rather than show "Pasting..." for ever.
                    self.clips.read = None;
                    self.show_toast("The other app did not hand over the picture. Try again.", 3.5);
                }
                Err(TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
                Err(TryRecvError::Disconnected) => {
                    self.clips.read = None;
                    self.show_toast("Could not paste the picture.", 3.0);
                }
            }
        }
        // A paste waits while the 3D view hides the drawing it is for.
        if let Some(rx) = self.clips.decode.as_ref().filter(|_| self.three_d.is_none()) {
            match rx.try_recv() {
                Ok(result) => {
                    self.clips.decode = None;
                    match result {
                        Ok(_) if self.clips.doc != self.session.document_generation() => {
                            self.show_toast("The paste was not added: another drawing was opened meanwhile.", 3.5);
                        }
                        Ok(pixels) => self.finish_foreign_paste(pixels),
                        Err(e) => self.show_toast(format!("Could not paste: {e}"), 4.0),
                    }
                }
                Err(TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
                Err(TryRecvError::Disconnected) => {
                    self.clips.decode = None;
                    self.show_toast("Could not paste the picture.", 3.0);
                }
            }
        }
    }

    /// Add the pasted picture as a "Paste" layer above the active one (one undo step) and start a transform.
    fn finish_foreign_paste(&mut self, pixels: Vec<u8>) {
        if self.transform.is_some() {
            self.show_toast("Finish or cancel the transform first.", 2.0);
            return;
        }
        match self.session.import_layer_named(Some(self.active), "Paste", pixels, "Paste") {
            Some(edit) => {
                self.apply_refresh(edit.refresh);
                self.needs_full = true;
                if let Some(id) = edit.id {
                    self.active = id;
                    self.begin_transform();
                }
                self.show_toast("Pasted", 1.0);
            }
            None => self.show_toast("Could not paste here.", 1.5),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip_of(pixels: Vec<[f32; 4]>, w: usize, h: usize) -> Clip {
        Clip { image: Image { width: w, height: h, pixels }, origin: Some((3, 4)) }
    }

    #[test]
    fn a_clip_round_trips_through_a_straight_alpha_png() {
        // Premultiplied layer bytes -> straight float -> PNG -> straight float -> premultiplied bytes.
        let premul: Vec<u8> = vec![
            255, 0, 0, 255, // opaque red
            0, 0, 0, 0, // clear
            100, 50, 25, 200, // translucent
            0, 255, 0, 255, // opaque green
        ];
        let image = Image::from_premul_rgba8(2, 2, &premul).unwrap();
        let clip = Clip { image, origin: None };
        let png = clip_to_png(&clip).unwrap();
        assert_eq!(&png[1..4], b"PNG");
        let back = png_to_image(&png).unwrap();
        assert_eq!((back.width, back.height), (2, 2));
        let out = back.to_premul_rgba8();
        // Exact at alpha 0 and alpha 255.
        assert_eq!(&out[0..4], &premul[0..4]);
        assert_eq!(&out[4..8], &premul[4..8]);
        assert_eq!(&out[12..16], &premul[12..16]);
        // The translucent pixel is within one step of the original after the round trip.
        for k in 0..4 {
            assert!((i32::from(out[8 + k]) - i32::from(premul[8 + k])).abs() <= 1, "{out:?}");
        }
    }

    #[test]
    fn the_png_is_straight_alpha_at_the_clips_own_size() {
        let clip = clip_of(vec![[1.0, 0.5, 0.0, 0.5]; 6], 3, 2);
        let png = clip_to_png(&clip).unwrap();
        let mut dec = png::Decoder::new(std::io::Cursor::new(&png));
        dec.set_transformations(png::Transformations::EXPAND);
        let mut reader = dec.read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buf).unwrap();
        assert_eq!((info.width, info.height), (3, 2));
        assert_eq!(&buf[..4], &[255, 128, 0, 128], "colour is not multiplied by alpha");
    }

    #[test]
    fn an_empty_clip_is_refused() {
        assert!(clip_to_png(&clip_of(Vec::new(), 0, 0)).is_err());
    }

    #[test]
    fn a_bad_png_is_an_error() {
        assert!(png_to_image(b"not a png").is_err());
    }

    #[test]
    fn writing_a_clip_replaces_older_clip_files() {
        let dir = std::env::temp_dir().join(format!("eskiz-clipdir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let clip = clip_of(vec![[0.0, 0.0, 1.0, 1.0]; 4], 2, 2);
        let a = write_clip_png(&dir, 1, &clip).unwrap();
        assert!(a.is_file());
        std::fs::write(dir.join("keep.txt"), b"x").unwrap();
        let b = write_clip_png(&dir, 2, &clip).unwrap();
        assert_eq!(b.file_name().unwrap(), "clip-2.png");
        assert!(b.is_file() && !a.exists());
        assert!(dir.join("keep.txt").exists(), "only clip files are removed");
        // An older copy that finishes last does not delete the newer file.
        let late = write_clip_png(&dir, 1, &clip).unwrap();
        assert!(b.is_file() && late.is_file(), "the newer clip survives an older write");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_desktop_clipboard_is_inert() {
        let c = NoSystemClipboard;
        assert_eq!(c.clip_dir(), None);
        assert_eq!(c.state(), ClipState::None);
        c.put_image(Path::new("/x.png"), 1);
        assert!(c.read_image().try_recv().unwrap().is_err());
    }

    #[test]
    fn the_mock_tracks_serials() {
        let m = MockSystemClipboard::new();
        assert_eq!(m.state(), ClipState::None);
        m.put_image(Path::new("/a.png"), 7);
        assert_eq!(m.state(), ClipState::Ours(7));
        m.copy_foreign(PathBuf::from("/b.png"));
        assert_eq!(m.state(), ClipState::ForeignImage);
        assert_eq!(m.read_image().try_recv().unwrap(), Ok(PathBuf::from("/b.png")));
    }
}
