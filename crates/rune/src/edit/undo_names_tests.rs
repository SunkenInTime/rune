use super::*;
use crate::{Ime, Key, KeyEvent, Mods};

fn key(e: &mut TextEdit, key: Key) {
    e.key(&KeyEvent { key, mods: Mods::default() }, None);
}

fn command(e: &mut TextEdit, c: char) {
    e.key(&KeyEvent { key: Key::Command(c), mods: Mods { cmd: true, ctrl: !cfg!(target_os = "macos"), ..Mods::default() } }, None);
}

fn names(e: &TextEdit) -> (Option<String>, Option<String>) {
    (e.undo_name().map(Into::into), e.redo_name().map(Into::into))
}

#[test]
fn typing_deleting_and_line_breaks_are_typing_and_redo_reads_the_name_undo_took() {
    let mut e = TextEdit::new("");
    assert!(!e.can_undo() && !e.can_redo());
    assert_eq!(names(&e), (None, None));
    for c in ["h", "i"] {
        key(&mut e, Key::Text(c.into()));
    }
    assert!(e.can_undo());
    assert_eq!(names(&e), (Some("Typing".into()), None));
    key(&mut e, Key::Backspace);
    assert_eq!(names(&e).0.as_deref(), Some("Typing"), "a deletion is typing too");
    e.undo();
    assert_eq!(names(&e), (Some("Typing".into()), Some("Typing".into())));
    e.set_single_line(false);
    e.key(&KeyEvent { key: Key::Enter, mods: Mods { shift: true, ..Mods::default() } }, None);
    assert_eq!(e.text, "h\n");
    assert_eq!(names(&e), (Some("Typing".into()), None), "a new edit clears redo");
}

#[test]
fn paste_cut_and_an_input_method_commit_are_named_as_appkit_names_them() {
    let mut e = TextEdit::new("one two");
    e.set_selection(0, 3);
    command(&mut e, 'x');
    assert_eq!(e.text, " two");
    assert_eq!(e.undo_name().as_deref(), Some("Cut"));
    e.paste("uno");
    assert_eq!(e.undo_name().as_deref(), Some("Paste"));
    e.set_caret(e.text.len());
    e.ime(&Ime::Preedit("に".into(), None));
    e.ime(&Ime::Commit("日本".into()));
    assert_eq!(e.text, "uno two日本");
    assert_eq!(e.undo_name().as_deref(), Some("Typing"));
    e.undo();
    assert_eq!((e.text.as_str(), e.undo_name().as_deref(), e.redo_name().as_deref()), ("uno two", Some("Paste"), Some("Typing")));
    e.undo();
    assert_eq!((e.text.as_str(), e.undo_name().as_deref(), e.redo_name().as_deref()), (" two", Some("Cut"), Some("Paste")));

    let mut rich = TextEdit::new("").rich();
    rich.paste_html("<b>bold</b>", "bold");
    assert_eq!(rich.undo_name().as_deref(), Some("Paste"), "a rich paste too");
    rich.undo();
    assert_eq!((rich.text.as_str(), rich.can_undo()), ("", false), "one step");
}

#[test]
fn an_apps_group_carries_its_name_and_rune_names_only_unnamed_groups() {
    let mut e = TextEdit::new("bold").rich();
    e.set_selection(0, 4);
    e.undo_group_named("Bold", |e| e.toggle_mark(Mark::Bold));
    assert_eq!(e.undo_name().as_deref(), Some("Bold"));

    e.undo_group_named("Replace All", |e| {
        e.set_selection(0, 4);
        e.paste("BOLD");
    });
    assert_eq!(e.undo_name().as_deref(), Some("Replace All"), "Rune's Paste doesn't rename the app's group");

    e.undo_group(|e| e.paste("x"));
    assert_eq!(e.undo_name().as_deref(), Some("Paste"), "an unnamed group takes the first name given inside it");

    e.undo_group(|e| {
        e.insert("y");
        e.set_undo_name("Draft");
        e.undo_group_named("Inner", |e| e.insert("z"));
        e.set_undo_name("Final");
    });
    assert_eq!(e.undo_name().as_deref(), Some("Final"), "set_undo_name replaces, a nested named group doesn't");
    e.set_undo_name("Outside");
    assert_eq!(e.undo_name().as_deref(), Some("Final"), "outside a group it does nothing");

    let before = e.text.clone();
    e.undo_group(|e| e.insert(" "));
    e.undo();
    assert_eq!(e.text, before);
    assert_eq!(e.undo_name().as_deref(), Some("Final"));

    e.undo_group(|e| e.set_block(Block::Quote));
    assert!(e.can_undo());
    assert_eq!(e.undo_name(), None, "an unnamed step has no name, though it can be undone");
}

#[test]
fn names_go_with_a_replaced_text_and_a_secure_field_has_none() {
    let mut e = TextEdit::new("");
    e.insert("a");
    e.replace_text("b");
    assert!(!e.can_undo() && e.undo_name().is_none());

    e.paste("c");
    e.text = "assigned".into();
    assert!(!e.can_undo() && e.undo_name().is_none(), "text the app assigned under the history isn't undone");

    let mut s = TextEdit::new("pw").secure();
    s.paste("x");
    assert!(!s.can_undo() && s.undo_name().is_none());
    s.undo_group_named("Bold", |s| {
        assert!(!s.can_undo(), "nothing to undo inside a group");
        s.insert("y");
    });
    assert!(s.undo_name().is_none());
}
