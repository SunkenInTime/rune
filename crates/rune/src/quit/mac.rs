//! AppKit asks the app delegate `applicationShouldTerminate:` before every quit: ⌘Q, the Dock's
//! Quit, logout, restart and shutdown. winit's delegate doesn't implement it, so Rune adds the
//! method to the delegate's class once (as `platform::reopen` does for reopening). It marks the
//! app quitting and lets the quit go ahead; `applicationWillTerminate:` follows at once, where
//! winit ends the loop and `Windows::exiting` runs every window's `App::on_quit`, then its
//! `on_close`.
//!
//! Once a window hears close requests (`App::on_close_request`), the user's Quit (a menu's Quit,
//! ⌘Q, the Dock's) is cancelled here and handed to the loop, which asks every window and quits
//! itself if they all agree, as `Tx::quit` does. A quit the system starts (logout, restart,
//! shutdown: the quit Apple event carries a reason) goes ahead without asking, as before.

use std::cell::Cell;
use std::ffi::{c_char, c_void};

use objc2::runtime::{AnyClass, AnyObject, Bool, Sel};
use objc2::{msg_send, sel};
use objc2_app_kit::NSApplication;
use objc2_foundation::MainThreadMarker;

thread_local! {
    /// The delegate class Rune added the method to.
    static HOOKED: Cell<*const AnyClass> = const { Cell::new(std::ptr::null()) };
    /// Hands the user's Quit to the loop; false when it couldn't.
    static ASK: std::cell::RefCell<Option<Box<dyn Fn() -> bool>>> = const { std::cell::RefCell::new(None) };
}

unsafe extern "C" {
    fn class_addMethod(class: *const AnyClass, selector: Sel, implementation: *const c_void, types: *const c_char) -> Bool;
}

/// `NSTerminateNow`.
const TERMINATE_NOW: usize = 1;
/// `NSTerminateCancel`.
const TERMINATE_CANCEL: usize = 0;
/// `keyAEQuitReason` ('why?'), the attribute a logout, restart or shutdown's quit event carries.
const QUIT_REASON: u32 = u32::from_be_bytes(*b"why?");

/// Whether the quit being asked about comes from the system (logout, restart, shutdown) rather
/// than the user's Quit: its Apple event says why.
fn system_quit() -> bool {
    let Some(class) = AnyClass::get("NSAppleEventManager") else { return false };
    // SAFETY: main thread. `sharedAppleEventManager` and `currentAppleEvent` return borrowed
    // objects or nil; `attributeDescriptorForKeyword:` takes an `AEKeyword` (`UInt32`) and
    // returns a descriptor or nil.
    unsafe {
        let manager: *mut AnyObject = msg_send![class, sharedAppleEventManager];
        let Some(manager) = manager.as_ref() else { return false };
        let event: *mut AnyObject = msg_send![manager, currentAppleEvent];
        let Some(event) = event.as_ref() else { return false };
        let reason: *mut AnyObject = msg_send![event, attributeDescriptorForKeyword: QUIT_REASON];
        !reason.is_null()
    }
}

extern "C" fn should_terminate(_: *mut AnyObject, _: Sel, _: *mut AnyObject) -> usize {
    // The loop asks the windows and quits itself if they agree (`Windows::ask_quit`).
    if super::ASKS.load(std::sync::atomic::Ordering::Relaxed)
        && !super::quitting()
        && !system_quit()
        && ASK.with(|ask| ask.borrow().as_ref().is_some_and(|ask| ask()))
    {
        return TERMINATE_CANCEL;
    }
    // Windows::exiting carries out the quit's effects.
    let _ = super::send(super::Event::Start { ending: false, after: super::After::Exit });
    TERMINATE_NOW
}

pub(super) fn install(ask: impl Fn() -> bool + 'static) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    ASK.with(|slot| *slot.borrow_mut() = Some(Box::new(ask)));
    let app = NSApplication::sharedApplication(mtm);
    let selector = sel!(applicationShouldTerminate:);
    // SAFETY: AppKit calls and runtime changes on the main thread. The added function has the
    // selector's ABI (`NSApplicationTerminateReply` is an `NSUInteger`, `Q` on 64-bit), and
    // adding a method the class lacks leaves its other methods and instances alone.
    unsafe {
        let delegate: *mut AnyObject = msg_send![&app, delegate];
        let Some(delegate) = delegate.as_ref() else { return };
        let class = delegate.class();
        if HOOKED.with(|hooked| std::ptr::eq(hooked.get(), class)) {
            return;
        }
        // A later winit, or an app's own delegate, decides itself.
        let answers: bool = msg_send![delegate, respondsToSelector: selector];
        if answers {
            return;
        }
        let imp = should_terminate as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject) -> usize;
        if !class_addMethod(class, selector, imp as *const c_void, c"Q@:@".as_ptr()).as_bool() {
            eprintln!("rune: could not add the terminate method to the app delegate");
            return;
        }
        HOOKED.with(|hooked| hooked.set(class));
        // AppKit caches which optional methods its delegate answers when it is set.
        let _: () = msg_send![&app, setDelegate: delegate];
    }
}
