//! The OS half of pen tablet input: a [`PenSource`] over the `octotablet` crate.
//!
//! Support in octotablet 0.1: Windows (Windows Ink RealTimeStylus) and Linux Wayland
//! (`tablet_unstable_v2`). X11 is not supported by the crate; there `connect` returns an error
//! and the app keeps using mouse and touch. macOS uses its own reader (`tablet_mac.rs`), because
//! the crate does not compile there. Elsewhere `connect` reports why there is no pen.

/// Connect to the OS tablet API using the window's raw handles. On any failure the error
/// text is meant for the preferences panel; the app carries on without a pen.
#[cfg(not(any(
    target_os = "android",
    all(target_os = "macos", feature = "tablet"),
    all(
        feature = "tablet",
        any(windows, all(unix, not(any(target_os = "macos", target_os = "ios", target_os = "android", target_os = "redox"))))
    )
)))]
pub fn connect(_cc: &eframe::CreationContext<'_>) -> Result<Box<dyn crate::tablet::PenSource>, String> {
    Err(if cfg!(feature = "tablet") {
        "no tablet support on this platform yet (mouse and touch work)".to_owned()
    } else {
        "not built in (cargo feature `tablet`)".to_owned()
    })
}

/// macOS: an `NSEvent` local monitor for tablet point and proximity events (Wacom-style
/// tablets, Sidecar with an Apple Pencil); see `tablet_mac.rs`.
#[cfg(all(target_os = "macos", feature = "tablet"))]
pub fn connect(_cc: &eframe::CreationContext<'_>) -> Result<Box<dyn crate::tablet::PenSource>, String> {
    Ok(Box::new(crate::tablet_mac::MacSource::new()?))
}

/// Android: the Java shim (`EskizActivity`) forwards `MotionEvent`s over JNI; see
/// `android_input.rs`. Stylus and eraser become pen events, fingers and mice become egui events.
#[cfg(target_os = "android")]
pub fn connect(cc: &eframe::CreationContext<'_>) -> Result<Box<dyn crate::tablet::PenSource>, String> {
    let ctx = cc.egui_ctx.clone();
    crate::android_queue::set_waker(move || ctx.request_repaint());
    Ok(Box::new(crate::android_source::AndroidSource::new(crate::android_queue::global())))
}

#[cfg(all(
    feature = "tablet",
    any(windows, all(unix, not(any(target_os = "macos", target_os = "ios", target_os = "android", target_os = "redox"))))
))]
pub use imp::connect;

/// Windows: the pen backend can be connected after the first frames and again at run time
/// (Settings > Pen), so the handles are kept.
#[cfg(all(feature = "tablet", windows))]
pub use imp::{connect_later, handles, Handles};

#[cfg(all(
    feature = "tablet",
    any(windows, all(unix, not(any(target_os = "macos", target_os = "ios", target_os = "android", target_os = "redox"))))
))]
mod imp {
    use octotablet::events::{Event, FrameTimestamp, ToolEvent};
    use octotablet::tool::Type;
    use octotablet::{Backend, Builder, Manager};

    use crate::tablet::{PenEvent, PenPose, PenSource};

    struct OctoSource {
        manager: Manager,
        backend: Backend,
        last_pose: Option<octotablet::axis::Pose>,
        failed: bool,
    }

    /// Tools that are not pens: touch is handled by egui's touch events, and mice and
    /// lenses are pointers.
    fn is_pen(tool_type: Option<Type>) -> bool {
        !matches!(tool_type, Some(Type::Finger | Type::Mouse | Type::Lens | Type::Emulated))
    }

    /// Build the manager from raw handles. A panic or an error becomes the message for the
    /// preferences panel; the app carries on without a pen.
    fn build(handles: &Handles) -> Result<OctoSource, String> {
        let builder = Builder::new().emulate_tool_from_mouse(false);
        // octotablet 0.1 `unwrap`s some Windows Ink set-up failures, so a panic here must
        // not take the app down.
        let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // SAFETY: the handles come from the live eframe window. The `Manager` is dropped
            // when the source is dropped (`eframe::App::on_exit`), before the window and
            // display connection go away.
            #[allow(unsafe_code)]
            unsafe {
                builder.build_raw(handles)
            }
        }));
        match built {
            Ok(Ok(manager)) => {
                let backend = manager.backed();
                Ok(OctoSource { manager, backend, last_pose: None, failed: false })
            }
            Ok(Err(e)) => Err(format!("unsupported here ({e})")),
            Err(_) => Err("the tablet API failed to start".to_owned()),
        }
    }

    /// The window and display handles, copied out of the creation context.
    #[derive(Clone, Copy)]
    pub struct Handles {
        window: raw_window_handle::RawWindowHandle,
        display: raw_window_handle::RawDisplayHandle,
    }

    // SAFETY: plain handle values (an `HWND`, a display pointer). They are only turned back into
    // handles for the window that is alive while the source exists.
    #[allow(unsafe_code)]
    unsafe impl Send for Handles {}

    impl raw_window_handle::HasWindowHandle for &Handles {
        fn window_handle(&self) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
            // SAFETY: see `Handles`.
            #[allow(unsafe_code)]
            Ok(unsafe { raw_window_handle::WindowHandle::borrow_raw(self.window) })
        }
    }

    impl raw_window_handle::HasDisplayHandle for &Handles {
        fn display_handle(&self) -> Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
            // SAFETY: see `Handles`.
            #[allow(unsafe_code)]
            Ok(unsafe { raw_window_handle::DisplayHandle::borrow_raw(self.display) })
        }
    }

    pub fn handles(cc: &eframe::CreationContext<'_>) -> Result<Handles, String> {
        use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
        let window = cc.window_handle().map_err(|e| format!("no window handle ({e})"))?.as_raw();
        let display = cc.display_handle().map_err(|e| format!("no display handle ({e})"))?.as_raw();
        Ok(Handles { window, display })
    }

    /// Linux (Wayland): on the UI thread, as before, because the Wayland event queue belongs to it.
    #[cfg(not(windows))]
    pub fn connect(cc: &eframe::CreationContext<'_>) -> Result<Box<dyn PenSource>, String> {
        let handles = handles(cc)?;
        build(&handles).map(|s| Box::new(s) as Box<dyn PenSource>)
    }

    /// Windows: Windows Ink runs on its own thread, see `ink_thread`.
    #[cfg(windows)]
    pub fn connect(cc: &eframe::CreationContext<'_>) -> Result<Box<dyn PenSource>, String> {
        let handles = handles(cc)?;
        connect_later(cc.egui_ctx.clone(), handles)
    }

    #[cfg(windows)]
    pub fn connect_later(ctx: eframe::egui::Context, handles: Handles) -> Result<Box<dyn PenSource>, String> {
        ink_thread::start(ctx, handles).map(|s| Box::new(s) as Box<dyn PenSource>)
    }

    impl PenSource for OctoSource {
        fn poll(&mut self) -> Vec<PenEvent> {
            let mut out = Vec::new();
            // The backend can panic inside `pump` as well as while it is set up.
            let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.drain(&mut out)));
            if ran.is_err() {
                self.failed = true;
                out.clear();
            }
            out
        }

        fn failed(&self) -> bool {
            self.failed
        }

        fn name(&self) -> &'static str {
            match self.backend {
                Backend::WaylandTabletUnstableV2 => "Wayland tablet protocol",
                Backend::WindowsInkRealTimeStylus => "Windows Ink",
            }
        }

        fn needs_pointer_events(&self) -> bool {
            // On Wayland a client that binds the tablet protocol may not get the pen as
            // pointer events; on Windows Ink the pen is also delivered as mouse input.
            matches!(self.backend, Backend::WaylandTabletUnstableV2)
        }
    }

    impl OctoSource {
        fn drain(&mut self, out: &mut Vec<PenEvent>) {
            // (`PumpError` has no variants on Windows, so match it through `ok()`.)
            let Some(events) = self.manager.pump().ok() else {
                return;
            };
            for event in events {
                let Event::Tool { tool, event } = event else {
                    continue;
                };
                if !is_pen(tool.tool_type) {
                    continue;
                }
                match event {
                    ToolEvent::In { .. } => {
                        self.last_pose = None;
                        out.push(PenEvent::Enter { eraser: tool.tool_type == Some(Type::Eraser) });
                    }
                    ToolEvent::Down => out.push(PenEvent::Down),
                    ToolEvent::Up => out.push(PenEvent::Up),
                    ToolEvent::Out | ToolEvent::Removed => out.push(PenEvent::Leave),
                    ToolEvent::Pose(p) => self.last_pose = Some(p),
                    // A frame closes one group of axis changes, so this is where a sample is made.
                    ToolEvent::Frame(ts) => {
                        if let Some(p) = self.last_pose {
                            out.push(PenEvent::Pose(PenPose {
                                pos: p.position,
                                pressure: p.pressure.get(),
                                tilt: p.tilt,
                                roll: None,
                                time: ts.map(|t| (t - FrameTimestamp::epoch()).as_secs_f64()),
                            }));
                        }
                    }
                    ToolEvent::Added | ToolEvent::Button { .. } => {}
                }
            }
        }
    }

    /// Windows Ink on its own thread.
    ///
    /// octotablet 0.1 creates the `IRealTimeStylus` on the calling thread, which in this app is
    /// the window thread that winit has made a COM single-threaded apartment (`OleInitialize`).
    /// Its plugin runs on the stylus's own thread and, while it holds the shared-state mutex
    /// (`com_impl.rs`, `RealTimeStylusEnabled` and `TabletAdded`), calls back into the stylus
    /// object; `pump` on the window thread takes the same mutex (`ink/mod.rs`, `pump`). Whenever
    /// the stylus object is bound to the window thread's apartment, that is a lock cycle through
    /// the message loop. Here the whole backend lives on one thread of its own, in the
    /// multi-threaded apartment, with no window and no message loop of ours to wait for, and the
    /// window thread only reads a channel. If Windows Ink hangs, only this thread does.
    #[cfg(windows)]
    mod ink_thread {
        use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
        use std::sync::mpsc::{channel, Receiver};
        use std::sync::{Arc, OnceLock};
        use std::time::{Duration, Instant};

        use super::{build, Handles};
        use crate::tablet::{PenEvent, PenSource};

        /// How long the window thread waits for the backend to come up.
        const SETUP_WAIT: Duration = Duration::from_millis(2500);
        /// A pump that has not finished for this long counts as hung.
        const HUNG_AFTER_MS: u64 = 3000;
        const TICK: Duration = Duration::from_millis(4);
        const CLOSE_WAIT: Duration = Duration::from_millis(500);

        #[allow(unsafe_code)]
        #[link(name = "ole32")]
        extern "system" {
            fn CoInitializeEx(reserved: *const core::ffi::c_void, co_init: u32) -> i32;
        }

        fn now_ms() -> u64 {
            static EPOCH: OnceLock<Instant> = OnceLock::new();
            u64::try_from(EPOCH.get_or_init(Instant::now).elapsed().as_millis()).unwrap_or(u64::MAX)
        }

        #[derive(Default)]
        struct Shared {
            stop: AtomicBool,
            done: AtomicBool,
            failed: AtomicBool,
            /// `now_ms` when the worker last went round its loop.
            beat_ms: AtomicU64,
        }

        pub struct ThreadSource {
            rx: Receiver<Vec<PenEvent>>,
            shared: Arc<Shared>,
            hung_logged: bool,
        }

        pub fn start(ctx: eframe::egui::Context, handles: Handles) -> Result<ThreadSource, String> {
            let started = Instant::now();
            let shared = Arc::new(Shared::default());
            shared.beat_ms.store(now_ms(), Ordering::Relaxed);
            let (setup_tx, setup_rx) = channel::<Result<(), String>>();
            let (tx, rx) = channel::<Vec<PenEvent>>();
            let worker = Arc::clone(&shared);
            std::thread::Builder::new()
                .name("eskiz-pen".into())
                .spawn(move || run(&worker, handles, &setup_tx, &tx, &ctx))
                .map_err(|e| format!("could not start the pen thread ({e})"))?;
            match setup_rx.recv_timeout(SETUP_WAIT) {
                Ok(Ok(())) => {
                    crate::startup::log(&format!("pen: Windows Ink connected on its own thread in {} ms", started.elapsed().as_millis()));
                    Ok(ThreadSource { rx, shared, hung_logged: false })
                }
                Ok(Err(e)) => Err(e),
                Err(_) => {
                    // The thread is left to finish or hang on its own; it drops the backend if it
                    // ever comes back.
                    shared.stop.store(true, Ordering::Relaxed);
                    crate::startup::log("pen: Windows Ink did not answer within 2.5 s; pen input is off for this run");
                    Err("Windows Ink did not answer in time, so pen input is off (mouse and touch work)".to_owned())
                }
            }
        }

        fn run(
            shared: &Shared,
            handles: Handles,
            setup: &std::sync::mpsc::Sender<Result<(), String>>,
            out: &std::sync::mpsc::Sender<Vec<PenEvent>>,
            ctx: &eframe::egui::Context,
        ) {
            // The multi-threaded apartment: nothing here depends on a message loop.
            #[allow(unsafe_code)]
            // SAFETY: plain call on a thread that has no COM state yet.
            let hr = unsafe { CoInitializeEx(std::ptr::null(), 0) };
            if hr < 0 {
                let _ = setup.send(Err(format!("COM could not start on the pen thread (0x{hr:08x})")));
                shared.done.store(true, Ordering::Relaxed);
                return;
            }
            let mut source = match build(&handles) {
                Ok(s) => s,
                Err(e) => {
                    let _ = setup.send(Err(e));
                    shared.done.store(true, Ordering::Relaxed);
                    return;
                }
            };
            if setup.send(Ok(())).is_err() {
                // The window thread gave up waiting.
                shared.done.store(true, Ordering::Relaxed);
                return;
            }
            while !shared.stop.load(Ordering::Relaxed) {
                shared.beat_ms.store(now_ms(), Ordering::Relaxed);
                let mut events = Vec::new();
                let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| source.drain(&mut events)));
                if ran.is_err() {
                    shared.failed.store(true, Ordering::Relaxed);
                    break;
                }
                if !events.is_empty() {
                    if out.send(events).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
                std::thread::sleep(TICK);
            }
            drop(source);
            shared.done.store(true, Ordering::Relaxed);
        }

        impl PenSource for ThreadSource {
            fn poll(&mut self) -> Vec<PenEvent> {
                if !self.hung_logged && !self.shared.done.load(Ordering::Relaxed) {
                    let age = now_ms().saturating_sub(self.shared.beat_ms.load(Ordering::Relaxed));
                    if age > HUNG_AFTER_MS {
                        self.hung_logged = true;
                        self.shared.failed.store(true, Ordering::Relaxed);
                        crate::startup::log(&format!("pen: the Windows Ink thread has not answered for {age} ms; pen input stopped, the window is not affected"));
                    }
                }
                self.rx.try_iter().flatten().collect()
            }

            fn failed(&self) -> bool {
                self.shared.failed.load(Ordering::Relaxed)
            }

            fn name(&self) -> &'static str {
                "Windows Ink"
            }
        }

        impl Drop for ThreadSource {
            fn drop(&mut self) {
                self.shared.stop.store(true, Ordering::Relaxed);
                // Give the worker a moment to release the stylus before the window goes away;
                // a hung worker is left behind (never joined).
                let until = Instant::now() + CLOSE_WAIT;
                while !self.shared.done.load(Ordering::Relaxed) && Instant::now() < until {
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }
    }
}
