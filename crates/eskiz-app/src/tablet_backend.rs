//! The OS half of pen tablet input: a [`PenSource`] over the `octotablet` crate.
//!
//! Support in octotablet 0.1: Windows (Windows Ink RealTimeStylus) and Linux Wayland
//! (`tablet_unstable_v2`). X11 and macOS are not supported by the crate; there `connect`
//! returns an error and the app keeps using mouse and touch. The crate itself does not
//! compile on macOS, so it is only a dependency on Windows and Unix-without-macOS, and
//! the whole module is behind the `tablet` cargo feature on those targets. Elsewhere
//! `connect` reports why there is no pen.

/// Connect to the OS tablet API using the window's raw handles. On any failure the error
/// text is meant for the preferences panel; the app carries on without a pen.
#[cfg(not(any(
    target_os = "android",
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
    }

    /// Tools that are not pens: touch is handled by egui's touch events, and mice and
    /// lenses are pointers.
    fn is_pen(tool_type: Option<Type>) -> bool {
        !matches!(tool_type, Some(Type::Finger | Type::Mouse | Type::Lens | Type::Emulated))
    }

    pub fn connect(cc: &eframe::CreationContext<'_>) -> Result<Box<dyn PenSource>, String> {
        let builder = Builder::new().emulate_tool_from_mouse(false);
        // octotablet 0.1 `unwrap`s some Windows Ink set-up failures, so a panic here must
        // not take the app down.
        let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // SAFETY: the handles come from the live eframe window. The `Manager` is stored
            // in the app and dropped in `eframe::App::on_exit` (and with the app), before the
            // window and display connection go away.
            #[allow(unsafe_code)]
            unsafe {
                builder.build_raw(cc)
            }
        }));
        match built {
            Ok(Ok(manager)) => {
                let backend = manager.backed();
                Ok(Box::new(OctoSource { manager, backend, last_pose: None }))
            }
            Ok(Err(e)) => Err(format!("unsupported here ({e})")),
            Err(_) => Err("the tablet API failed to start".to_owned()),
        }
    }

    impl PenSource for OctoSource {
        fn poll(&mut self) -> Vec<PenEvent> {
            let mut out = Vec::new();
            // (`PumpError` has no variants on Windows, so match it through `ok()`.)
            let Some(events) = self.manager.pump().ok() else {
                return out;
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
                                time: ts.map(|t| (t - FrameTimestamp::epoch()).as_secs_f64()),
                            }));
                        }
                    }
                    ToolEvent::Added | ToolEvent::Button { .. } => {}
                }
            }
            out
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
}
