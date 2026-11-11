//! Refusing a close: the window has unsaved changes, so its close button, Alt+F4, ⌘W or the
//! window manager's Close shows an alert in the window instead of closing it. Don't Save closes
//! it with `Tx::close_window`, which doesn't ask again. Headless: `--script` with
//! `{"close": true}`.
use rune::*;

#[derive(Default)]
struct Editor { refused: u32, alert: bool }

fn button(key: &str, title: &str) -> El {
    row().key(key).padding(8.0).radius(6.0).fill(oklch(0.45, 0.02, 260.0)).child(text(title))
}

fn view(s: &Editor, _: &mut Cx<Editor>) -> El {
    let mut root = col().padding(24.0).gap(16.0)
        .child(text(format!("Unsaved changes. Closes refused: {}", s.refused)));
    if s.alert {
        root = root.child(text("Save changes before closing?"))
            .child(row().gap(12.0)
                .child(button("dont-save", "Don't Save").on_press_tx(|_: &mut Editor, tx: &mut Tx| tx.close_window()))
                .child(button("cancel", "Cancel").on_press(|s: &mut Editor| s.alert = false)));
    }
    root
}

fn main() {
    App::new(AppSpec::new("Rune close request", (360.0, 200.0), Editor::default, view))
        .on_close_request(|s, _, _| { s.refused += 1; s.alert = true; false })
        .run();
}
