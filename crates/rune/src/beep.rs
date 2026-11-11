//! The system alert sound, for an action the app refuses (AppKit's `NSBeep`): [`Tx::beep`].
//! A window plays it; a headless run or test only counts it and records `beep` in `--trace`.
use crate::Tx;

/// A runtime's beeps: whether it plays them, and how many it was asked for.
#[derive(Default)]
pub(crate) struct Beeps {
    /// A native window that plays sounds, not a headless run, test or trace.
    pub(crate) plays: bool,
    pub(crate) count: u32,
}

impl Tx {
    /// Plays the system's alert sound once this handler's turn is done, as AppKit beeps when a
    /// key or command can't do anything (Delete at the start of a field, a disabled shortcut):
    /// `NSBeep` on macOS, `MessageBeep(MB_OK)` on Windows, and on Linux the X11 bell (which the
    /// desktop turns into its alert sound or a visual bell, as the user set it) for an X11
    /// window, else the sound theme's `bell` through `canberra-gtk-play` when it's installed,
    /// else nothing. Several in one turn play once. Headless runs and tests don't play it:
    /// they count it (`Runtime::beeps`) and record `beep` in `--trace`. No sound on the web.
    pub fn beep(&mut self) { self.beep = true; }
}

impl<S: 'static> crate::Runtime<S> {
    /// How many turns asked for [`Tx::beep`] since the runtime started, played or not.
    pub fn beeps(&self) -> u32 { self.beeps.count }

    pub(crate) fn beep(&mut self) {
        self.beeps.count += 1;
        self.trace_event("beep");
        if self.beeps.plays { play(); }
    }
}

#[cfg(target_os = "macos")]
fn play() {
    // SAFETY: `NSBeep` takes no arguments; handlers run on the main thread.
    unsafe { objc2_app_kit::NSBeep() }
}

#[cfg(target_os = "windows")]
fn play() {
    use windows_sys::Win32::{System::Diagnostics::Debug::MessageBeep, UI::WindowsAndMessaging::MB_OK};
    // SAFETY: plain call; it queues the sound and returns.
    unsafe { MessageBeep(MB_OK) };
}

/// Off the event thread: connecting to X or starting a process can stall.
#[cfg(all(unix, not(any(target_os = "macos", target_os = "ios", target_os = "android"))))]
fn play() {
    let _ = std::thread::Builder::new().name("rune-beep".into()).spawn(|| {
        if std::env::var_os("DISPLAY").is_some() && x11_bell() { return; }
        canberra_bell();
    });
}

/// The core protocol's bell at the user's volume. With XKB (every current server) it reaches
/// the window manager as an `XkbBellNotify`, which GNOME, KDE and others play as the sound
/// theme's alert or flash as a visual bell.
#[cfg(all(unix, not(any(target_os = "macos", target_os = "ios", target_os = "android"))))]
fn x11_bell() -> bool {
    let Ok((conn, _)) = x11rb::connect(None) else { return false };
    // `check` waits for the server to take it before the connection closes.
    x11rb::protocol::xproto::bell(&conn, 0).is_ok_and(|sent| sent.check().is_ok())
}

/// The sound theme's `bell` event, for Wayland sessions with no X server.
#[cfg(all(unix, not(any(target_os = "macos", target_os = "ios", target_os = "android"))))]
fn canberra_bell() {
    let _ = std::process::Command::new("canberra-gtk-play")
        .args(["-i", "bell", "-d", "Alert"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

#[cfg(not(any(target_os = "macos", target_os = "windows", all(unix, not(any(target_os = "ios", target_os = "android"))))))]
fn play() {}

#[cfg(test)]
#[path = "beep_tests.rs"]
mod tests;
