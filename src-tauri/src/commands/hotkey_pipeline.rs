//! Thin hotkey adapter.
//!
//! All pipeline orchestration lives in [`super::recording_session`]; this module
//! only builds the callback that delegates press/release to a [`RecordingSession`].
//! `make_hotkey_callback(PipelineState) -> HotkeyCallback` keeps its signature so
//! call sites ([`crate::lib`] / [`super::config_cmd`]) are unchanged.

use crate::hotkey::{HotkeyCallback, HotkeyEvent};

use super::pipeline_state::PipelineState;
use super::recording_session::RecordingSession;

/// Build the hotkey callback that starts/stops recording and runs the full
/// pipeline via a [`RecordingSession`].
pub(crate) fn make_hotkey_callback(ps: PipelineState) -> HotkeyCallback {
    let session = RecordingSession::new(ps);
    Box::new(move |event| match event {
        HotkeyEvent::Pressed => session.on_press(),
        HotkeyEvent::Released => session.handle_release(),
    })
}
