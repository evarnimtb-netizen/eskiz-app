//! Playback clock and playhead.
//!
//! A frame is shown for `hold + 1` ticks. On each tick the frame is either held, or the playhead
//! moves on as the playback mode says. The clock turns elapsed seconds into whole ticks at the frame
//! rate and carries the leftover fraction forward, so the average rate is exact.

use crate::frames::FrameList;
use crate::settings::{clamp_frame_rate, AnimationSettings, PlaybackDirection, PlaybackMode};

/// What one tick did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tick {
    /// The frame is still inside its hold.
    Held,
    /// A new visit started. At a ping-pong turnaround the frame index is unchanged, but the visit is new.
    Moved,
    /// One-shot reached its end. Playback stopped on the end frame.
    Stopped,
}

/// The playhead, its direction, and the playback clock.
#[derive(Clone, Debug, PartialEq)]
pub struct Playback {
    mode: PlaybackMode,
    /// The direction the settings start in. Used when a finished playback starts again.
    start_forward: bool,
    frame: usize,
    forward: bool,
    /// Ticks spent on the current frame visit so far.
    ticks_in_visit: u32,
    playing: bool,
    finished: bool,
    fps: u32,
    /// Fraction of a tick carried to the next clock update.
    carry: f64,
}

impl Playback {
    /// A stopped playhead on `frame`, using the mode, direction and frame rate of `settings`.
    pub fn new(settings: &AnimationSettings, frame: usize) -> Self {
        let forward = settings.direction == PlaybackDirection::Forward;
        Self {
            mode: settings.playback_mode,
            start_forward: forward,
            frame,
            forward,
            ticks_in_visit: 0,
            playing: false,
            finished: false,
            fps: settings.fps(),
            carry: 0.0,
        }
    }

    /// Current frame index.
    pub fn frame(&self) -> usize {
        self.frame
    }

    /// True while the playhead moves on the clock.
    pub fn is_playing(&self) -> bool {
        self.playing
    }

    /// True after a one-shot playback has reached its end.
    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// True when the playhead moves forward.
    pub fn is_forward(&self) -> bool {
        self.forward
    }

    /// Ticks spent on the current frame so far. 0 at the start of a visit.
    pub fn ticks_in_visit(&self) -> u32 {
        self.ticks_in_visit
    }

    /// Playback frame rate in use.
    pub fn fps(&self) -> u32 {
        self.fps
    }

    /// Sets the playback frame rate, clamped to 1 to 60. Applies from the next clock update.
    pub fn set_fps(&mut self, fps: u32) {
        self.fps = clamp_frame_rate(fps);
    }

    /// Starts playing. The current frame is shown at once.
    ///
    /// After a one-shot has finished, this starts again from the first frame in the playback direction.
    /// Leaves the reset open, so this is a `[choice]`.
    pub fn play(&mut self, frames: &FrameList) {
        if frames.is_empty() {
            self.playing = false;
            return;
        }
        if self.finished {
            self.finished = false;
            self.forward = self.start_forward;
            self.frame = if self.start_forward {
                0
            } else {
                frames.len() - 1
            };
            self.ticks_in_visit = 0;
        }
        self.carry = 0.0;
        self.playing = true;
    }

    /// Pauses. The playhead stays on the current frame.
    pub fn pause(&mut self) {
        self.playing = false;
        self.carry = 0.0;
    }

    /// Sets the frame directly and pauses playback. An index past the end
    /// selects the last frame.
    pub fn scrub(&mut self, frame: usize, frames: &FrameList) {
        self.playing = false;
        self.finished = false;
        self.carry = 0.0;
        self.frame = frame.min(frames.len().saturating_sub(1));
        self.ticks_in_visit = 0;
    }

    /// One tick, as a step button does. Stepping pauses playback.
    pub fn step(&mut self, frames: &FrameList) -> Tick {
        self.playing = false;
        self.carry = 0.0;
        self.tick(frames)
    }

    /// Advances the clock by `dt_seconds` and runs every whole tick that has elapsed.
    ///
    /// Returns the number of ticks run. Does nothing unless playing, and stops early at a one-shot end.
    pub fn advance(&mut self, dt_seconds: f64, frames: &FrameList) -> u32 {
        if !self.playing || dt_seconds.is_nan() || dt_seconds <= 0.0 {
            return 0;
        }
        self.carry += dt_seconds * f64::from(self.fps);
        // The small epsilon keeps a period that is exact in theory from losing a tick to rounding.
        let whole = (self.carry + 1e-9).floor();
        if whole < 1.0 {
            return 0;
        }
        self.carry -= whole;
        let mut ran = 0;
        for _ in 0..(whole as u64) {
            if self.tick(frames) == Tick::Stopped {
                break;
            }
            ran += 1;
        }
        if !self.playing {
            self.carry = 0.0;
        }
        ran
    }

    /// Runs one tick on the playhead. Works whether or not the clock is running.
    fn tick(&mut self, frames: &FrameList) -> Tick {
        let n = frames.len();
        if n == 0 || self.finished {
            self.playing = false;
            return Tick::Stopped;
        }
        if self.frame >= n {
            self.frame = n - 1;
        }
        self.ticks_in_visit += 1;
        if self.ticks_in_visit <= frames.hold(self.frame) {
            return Tick::Held;
        }
        self.ticks_in_visit = 0;
        let last = n - 1;
        match self.mode {
            PlaybackMode::Loop => {
                self.frame = if self.forward {
                    (self.frame + 1) % n
                } else if self.frame == 0 {
                    last
                } else {
                    self.frame - 1
                };
            }
            PlaybackMode::PingPong => {
                if self.forward && self.frame == last {
                    // The last frame is shown once more, reversed.
                    self.forward = false;
                } else if !self.forward && self.frame == 0 {
                    // The first frame is shown again, going forward.
                    self.forward = true;
                } else if self.forward {
                    self.frame += 1;
                } else {
                    self.frame -= 1;
                }
            }
            PlaybackMode::OneShot => {
                let at_end = if self.forward {
                    self.frame == last
                } else {
                    self.frame == 0
                };
                if at_end {
                    self.finished = true;
                    self.playing = false;
                    return Tick::Stopped;
                }
                self.frame = if self.forward {
                    self.frame + 1
                } else {
                    self.frame - 1
                };
            }
        }
        Tick::Moved
    }
}
