//! The user asking to close the window, which the app may refuse (`App::on_close_request`): an
//! editor with unsaved changes keeps its window up and shows its alert, as AppKit's
//! `windowShouldClose:` and `applicationShouldTerminate:` let a document app do. Closes the app
//! asks for itself (`Tx::close_window`, `AppHandle::close`, `WindowHandle::close`) never ask.
use super::*;

/// What the user asked for when `App::on_close_request` runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseRequest {
    /// Close this window: its close button, ⌘W (the standard Window › Close item or a ⌘W
    /// nothing took), Alt+F4, the window manager's Close, a script's `{"close": true}`.
    Window,
    /// Quit the app (macOS ⌘Q from a menu's Quit item, the Dock's Quit): every window is asked
    /// in turn, and the first that refuses keeps the app running.
    Quit,
}

impl<S: 'static> Runtime<S> {
    /// Asks the app whether a close the user asked for goes ahead: runs `App::on_close_request`
    /// as an update and returns its answer, true without the hook. The caller closes (or
    /// quits) on true; on false the window stays as it was. What the hook asks of its `Tx`
    /// happens either way, so a hook that calls `Tx::close_window` closes the window whatever
    /// it returns. Hosts call it for the user's closes; tests and headless drivers may too.
    pub fn request_close(&mut self, why: CloseRequest) -> bool {
        let Some(f) = self.on_close_request else { return true };
        let mut proceed = true;
        self.update(|s, tx| proceed = f(s, why, tx));
        self.trace_event(format_args!("close-request:{}:{}", match why {
            CloseRequest::Window => "window",
            CloseRequest::Quit => "quit",
        }, if proceed { "allowed" } else { "refused" }));
        proceed
    }
}

#[cfg(test)]
#[path = "close_request_tests.rs"]
mod tests;
