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

/// Combines the config and environment caps by enforcing the slower effective interval.
///
/// The environment setting remains an optional cap; the experimental config cap is always active
/// for the client-owned shell.
pub(super) fn effective_presentation_interval(
    config_interval: Duration,
    env_interval: Option<Duration>,
) -> Duration {
    env_interval.map_or(config_interval, |env_interval| {
        config_interval.max(env_interval)
    })
}

/// Coalesces shell frames while a client presentation deadline is outstanding, retaining graphics
/// side effects in order for the eventual latest frame.
pub(super) struct SemanticFramePacer {
    interval: Duration,
    last_present_at: Option<Instant>,
    next_present_at: Option<Instant>,
    pending_frame: Option<FrameData>,
    pending_graphics: Vec<u8>,
    pending_scope: Option<u64>,
}

impl SemanticFramePacer {
    pub(super) fn new(interval: Duration) -> Self {
        Self {
            interval,
            last_present_at: None,
            next_present_at: None,
            pending_frame: None,
            pending_graphics: Vec::new(),
            pending_scope: None,
        }
    }

    /// Update the cap without discarding the latest coalesced frame.
    pub(super) fn update_interval(&mut self, interval: Duration) {
        self.interval = interval;
        if let Some(last_present_at) = self.last_present_at {
            self.next_present_at = Some(last_present_at + interval);
        }
    }

    /// Submit a frame without scope tracking for tests and non-graphics callers.
    #[cfg(test)]
    pub(super) fn submit(&mut self, frame: FrameData, now: Instant) -> Option<FrameData> {
        self.submit_scoped(frame, now, None)
    }

    /// Submit a frame while retaining graphics side effects from frames coalesced before its
    /// deadline. A scope change drops the old graphics journal before it can reach the host.
    pub(super) fn submit_scoped(
        &mut self,
        mut frame: FrameData,
        now: Instant,
        scope: Option<u64>,
    ) -> Option<FrameData> {
        self.discard_if_scope_changed(scope);
        if self.next_present_at.is_none_or(|deadline| now >= deadline) {
            self.prepend_pending_graphics(&mut frame);
            self.last_present_at = Some(now);
            self.next_present_at = Some(now + self.interval);
            self.pending_frame = None;
            Some(frame)
        } else {
            self.move_pending_graphics();
            if !frame.graphics.is_empty() {
                self.remember_pending_scope(scope);
            }
            self.pending_frame = Some(frame);
            None
        }
    }

    pub(super) fn presentation_deadline(&self) -> Option<Instant> {
        self.pending_frame.as_ref().and(self.next_present_at)
    }

    #[cfg(test)]
    pub(super) fn present_due(&mut self, now: Instant) -> Option<FrameData> {
        self.present_due_scoped(now, None)
    }

    pub(super) fn present_due_scoped(
        &mut self,
        now: Instant,
        scope: Option<u64>,
    ) -> Option<FrameData> {
        self.discard_if_scope_changed(scope);
        let deadline = self.presentation_deadline()?;
        if now < deadline {
            return None;
        }

        let mut frame = self.pending_frame.take()?;
        self.prepend_pending_graphics(&mut frame);
        self.last_present_at = Some(now);
        self.next_present_at = Some(now + self.interval);
        Some(frame)
    }

    /// Queue direct graphics behind the pending paced frame, or while frozen when forced.
    pub(super) fn queue_graphics(
        &mut self,
        graphics: &[u8],
        scope: Option<u64>,
        force: bool,
    ) -> bool {
        if graphics.is_empty() {
            return false;
        }
        self.discard_if_scope_changed(scope);
        if !force && self.pending_frame.is_none() && self.pending_graphics.is_empty() {
            return false;
        }
        self.move_pending_graphics();
        self.remember_pending_scope(scope);
        self.pending_graphics.extend_from_slice(graphics);
        true
    }

    /// Take queued graphics for a handoff flush while preserving any pending frame cells.
    pub(super) fn take_pending_graphics(&mut self) -> Option<Vec<u8>> {
        self.move_pending_graphics();
        if self.pending_graphics.is_empty() {
            return None;
        }
        self.pending_scope = None;
        Some(std::mem::take(&mut self.pending_graphics))
    }

    /// Extract old-scope graphics before dropping them, so they can be written before the new
    /// scope's frame and its cleanup commands.
    pub(super) fn take_scope_mismatch_graphics(&mut self, scope: Option<u64>) -> Option<Vec<u8>> {
        let mismatch = self
            .pending_scope
            .is_some_and(|pending_scope| Some(pending_scope) != scope);
        if !mismatch {
            return None;
        }
        self.move_pending_graphics();
        let graphics = std::mem::take(&mut self.pending_graphics);
        self.discard();
        (!graphics.is_empty()).then_some(graphics)
    }

    /// End the current presentation epoch while retaining side effects for the next frame.
    pub(super) fn clear(&mut self) {
        self.move_pending_graphics();
        self.last_present_at = None;
        self.next_present_at = None;
        self.pending_frame = None;
    }

    /// Drop the current frame and all graphics side effects when entering a different scope.
    pub(super) fn discard(&mut self) {
        self.last_present_at = None;
        self.next_present_at = None;
        self.pending_frame = None;
        self.pending_graphics.clear();
        self.pending_scope = None;
    }

    fn discard_if_scope_changed(&mut self, scope: Option<u64>) {
        if self
            .pending_scope
            .is_some_and(|pending_scope| Some(pending_scope) != scope)
        {
            self.discard();
        }
    }

    fn remember_pending_scope(&mut self, scope: Option<u64>) {
        if self.pending_scope.is_none() {
            self.pending_scope = scope;
        }
    }

    fn move_pending_graphics(&mut self) {
        let Some(pending) = self.pending_frame.as_mut() else {
            return;
        };
        if pending.graphics.is_empty() {
            return;
        }
        let graphics = std::mem::take(&mut pending.graphics);
        if self.pending_graphics.is_empty() {
            self.pending_graphics = graphics;
        } else {
            self.pending_graphics.extend(graphics);
        }
    }

    fn prepend_pending_graphics(&mut self, frame: &mut FrameData) {
        self.move_pending_graphics();
        if self.pending_graphics.is_empty() {
            return;
        }
        let mut graphics = std::mem::take(&mut self.pending_graphics);
        graphics.extend(std::mem::take(&mut frame.graphics));
        frame.graphics = graphics;
        self.pending_scope = None;
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
    /// Effective cap for client-shell semantic frame presentation.
    pub(super) presentation_interval: Option<Duration>,
    /// Environment cap retained so config reloads continue enforcing it.
    pub(super) env_presentation_interval: Option<Duration>,
    pub(super) shell_config: Option<shell::ClientShellConfig>,
}
