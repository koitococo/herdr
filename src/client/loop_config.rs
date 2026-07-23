use super::shell;
use std::time::{Duration, Instant};

use tracing::warn;

use crate::protocol::FrameData;

pub(super) const CLIENT_REFRESH_RATE_ENV_VAR: &str = "HERDR_CLIENT_REFRESH_RATE_HZ";

pub(super) fn client_refresh_interval_from_env() -> Option<Duration> {
    let value = match std::env::var(CLIENT_REFRESH_RATE_ENV_VAR) {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent) => return None,
        Err(std::env::VarError::NotUnicode(_)) => {
            warn!(
                "ignoring invalid HERDR_CLIENT_REFRESH_RATE_HZ; expected an integer from 1 through 60"
            );
            return None;
        }
    };

    let hertz = value
        .bytes()
        .all(|byte| byte.is_ascii_digit())
        .then(|| value.parse::<u64>().ok())
        .flatten()
        .filter(|hertz| (1..=60).contains(hertz));

    match hertz {
        Some(hertz) => Some(Duration::from_nanos(1_000_000_000 / hertz)),
        None => {
            warn!(
                "ignoring invalid HERDR_CLIENT_REFRESH_RATE_HZ; expected an integer from 1 through 60"
            );
            None
        }
    }
}

/// Coalesces shell frames while a client presentation deadline is outstanding.
pub(super) struct SemanticFramePacer {
    interval: Duration,
    next_present_at: Option<Instant>,
    pending_frame: Option<FrameData>,
}

impl SemanticFramePacer {
    pub(super) fn new(interval: Duration) -> Self {
        Self {
            interval,
            next_present_at: None,
            pending_frame: None,
        }
    }

    /// Present immediately when the current deadline has elapsed; otherwise retain only the
    /// newest frame for the next deadline.
    pub(super) fn submit(&mut self, frame: FrameData, now: Instant) -> Option<FrameData> {
        if self
            .next_present_at
            .map_or(true, |deadline| now >= deadline)
        {
            self.next_present_at = Some(now + self.interval);
            self.pending_frame = None;
            Some(frame)
        } else {
            self.pending_frame = Some(frame);
            None
        }
    }

    pub(super) fn presentation_deadline(&self) -> Option<Instant> {
        self.pending_frame.as_ref().and(self.next_present_at)
    }

    pub(super) fn present_due(&mut self, now: Instant) -> Option<FrameData> {
        let deadline = self.presentation_deadline()?;
        if now < deadline {
            return None;
        }

        self.next_present_at = Some(now + self.interval);
        self.pending_frame.take()
    }

    /// Discard frames from the previous presentation epoch, including its deadline.
    pub(super) fn clear(&mut self) {
        self.next_present_at = None;
        self.pending_frame = None;
    }
}

pub(super) struct ClientLoopConfig {
    pub(super) sound_config: crate::config::SoundConfig,
    pub(super) mouse_scroll_lines: usize,
    pub(super) redraw_on_focus_gained: bool,
    pub(super) host_cursor: crate::config::HostCursorModeConfig,
    pub(super) kitty_graphics_enabled: bool,
    pub(super) pixel_geometry_enabled: bool,
    pub(super) pixel_geometry_fallback: bool,
    pub(super) mouse_capture_active: bool,
    pub(super) endpoint_keybindings: bool,
    pub(super) remote_image_paste_key:
        Option<(crossterm::event::KeyCode, crossterm::event::KeyModifiers)>,
    /// Optional per-client cap for client-shell semantic frame presentation.
    pub(super) presentation_interval: Option<Duration>,
    pub(super) shell_config: Option<shell::ClientShellConfig>,
}
