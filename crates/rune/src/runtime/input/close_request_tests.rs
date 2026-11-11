use crate::el::Shell;
use crate::{CloseRequest, Cx, El, Env, Runtime, Tx, el};

/// An editor with unsaved changes: a close the user asks for shows its alert instead.
#[derive(Default)]
struct Editor {
    unsaved: bool,
    alert: bool,
    asked: Vec<CloseRequest>,
}

fn view(_: &Editor, _: &mut Cx<Editor>) -> El { el() }

fn ask(s: &mut Editor, why: CloseRequest, _: &mut Tx) -> bool {
    s.asked.push(why);
    if s.unsaved { s.alert = true; }
    !s.unsaved
}

fn editor(unsaved: bool) -> Runtime<Editor> {
    let mut rt = Runtime::new(Editor { unsaved, ..Default::default() }, view, (100.0, 100.0), Env::default());
    rt.on_close_request = Some(ask);
    rt
}

#[test]
fn without_the_hook_every_close_goes_ahead() {
    let mut rt = Runtime::new(Editor::default(), view, (100.0, 100.0), Env::default());
    assert!(rt.request_close(CloseRequest::Window));
    assert!(rt.request_close(CloseRequest::Quit));
    assert!(rt.shell_request.is_none(), "asking closes nothing itself; the host does");
}

#[test]
fn a_refused_close_keeps_the_window_and_a_forced_close_does_not_ask_again() {
    let mut rt = editor(true);
    assert!(!rt.request_close(CloseRequest::Window));
    assert!(rt.state.alert && rt.shell_request.is_none(), "the alert shows and nothing closes");
    assert!(rt.stale(), "the hook's change draws");
    // The user picks Don't Save in the alert.
    rt.update(|s, tx| { s.alert = false; tx.close_window(); });
    assert_eq!(rt.shell_request.take(), Some(Shell::Close));
    assert_eq!(rt.state.asked, [CloseRequest::Window], "close_window forces the close without asking");

    let mut clean = editor(false);
    assert!(clean.request_close(CloseRequest::Quit));
    assert_eq!(clean.state.asked, [CloseRequest::Quit]);
}

#[test]
fn a_hook_that_closes_itself_closes_whatever_it_returns() {
    let mut rt = Runtime::new(Editor::default(), view, (100.0, 100.0), Env::default());
    rt.on_close_request = Some(|_, _, tx| { tx.close_window(); false });
    assert!(!rt.request_close(CloseRequest::Window));
    assert_eq!(rt.shell_request, Some(Shell::Close));
}

#[test]
fn an_unhandled_command_w_asks_as_the_close_button_does() {
    use crate::runtime::Unhandled;
    use crate::{Key, KeyEvent, Mods, PhysicalInput, PhysicalKeyCode as Code};
    let cmd = Mods { cmd: true, ..Mods::default() };
    let press = |rt: &mut Runtime<Editor>| {
        rt.physical_then(PhysicalInput::Key { key: Code::KeyW, mods: cmd, pressed: true, repeat: false },
            Some(KeyEvent { key: Key::Command('w'), mods: cmd }), None, Some(Unhandled::Close));
        rt.frame(0.0);
    };
    let mut rt = editor(true);
    press(&mut rt);
    assert!(rt.shell_request.is_none() && rt.state.alert, "refused: the window stays and shows its alert");
    rt.state.unsaved = false;
    press(&mut rt);
    assert_eq!(rt.shell_request.take(), Some(Shell::Close), "allowed: it closes");
    assert_eq!(rt.state.asked, [CloseRequest::Window, CloseRequest::Window]);
}

#[test]
fn a_scripts_close_is_the_users_close() {
    let events = crate::tooling::load_script(&serde_json::json!([{ "t": 0.5, "close": true }]));
    assert!(matches!(events[0].kind, crate::tooling::Kind::Close));
    assert!(matches!(events[0].public().event, crate::tooling::ScriptEvent::Close));
}
