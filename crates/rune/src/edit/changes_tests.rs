use super::*;
use crate::{Cx, El, Env, Ime, Key, KeyEvent, Mods, Runtime, TextStyle, text_input};

fn key(e: &mut TextEdit, key: Key) {
    e.key(&KeyEvent { key, mods: Mods::default() }, None);
}

fn command(c: char) -> KeyEvent {
    KeyEvent { key: Key::Command(c), mods: Mods { cmd: true, ctrl: !cfg!(target_os = "macos"), ..Mods::default() } }
}

fn change(range: Range<usize>, inserted: usize) -> TextChange {
    TextChange::new(range, inserted)
}

/// The changes describe the edit from `old` to `new`: the lengths add up, and every byte of
/// `old` no change replaced, carried through them as an app carries an anchor, is the same
/// byte in `new`.
fn describes(old: &str, new: &str, changes: &[TextChange]) {
    let mut at: Vec<Option<usize>> = (0..old.len()).map(Some).collect();
    let mut len = old.len();
    for c in changes {
        assert!(c.range.start <= c.range.end && c.range.end <= len, "{c:?} in {len} bytes");
        for p in &mut at {
            *p = p.and_then(|q| if q < c.range.start { Some(q) } else if q >= c.range.end { Some(q - c.range.len() + c.inserted) } else { None });
        }
        len = len - c.range.len() + c.inserted;
    }
    assert_eq!(len, new.len(), "{old:?} -> {new:?} by {changes:?}");
    for (i, q) in at.iter().enumerate().filter_map(|(i, q)| Some((i, (*q)?))) {
        assert_eq!(old.as_bytes()[i], new.as_bytes()[q], "byte {i} of {old:?} is {q} of {new:?} by {changes:?}");
    }
}

/// Runs `f` on `e` and checks what it reported, returning it.
fn edit(e: &mut TextEdit, f: impl FnOnce(&mut TextEdit)) -> Vec<TextChange> {
    let old = e.text.clone();
    f(e);
    let changes = e.take_changes();
    describes(&old, &e.text, &changes);
    changes
}

#[test]
fn typing_deleting_and_line_breaks_report_each_change_and_undo_takes_back_the_run_whole() {
    let mut e = TextEdit::new("").report_changes();
    e.set_single_line(false);
    assert_eq!(edit(&mut e, |e| for c in ["h", "i"] { key(e, Key::Text(c.into())) }), [change(0..0, 1), change(1..1, 1)]);
    assert_eq!(edit(&mut e, |e| key(e, Key::Backspace)), [change(1..2, 0)]);
    let line_break = KeyEvent { key: Key::Enter, mods: Mods { shift: true, ..Mods::default() } };
    assert_eq!(edit(&mut e, |e| { e.key(&line_break, None); }), [change(1..1, 1)]);
    assert_eq!(e.text, "h\n");
    // The line break joined the deletion's step: undoing it puts the "i" back where the line
    // break is, as one change.
    assert_eq!(edit(&mut e, TextEdit::undo), [change(1..2, 1)]);
    assert_eq!(e.text, "hi");
    assert_eq!(edit(&mut e, TextEdit::undo), [change(0..2, 0)], "the typing run goes as one change");
    assert_eq!(edit(&mut e, TextEdit::redo), [change(0..0, 2)]);
    assert!(e.take_changes().is_empty(), "taken once");
}

#[test]
fn undo_reports_the_exact_inverse_where_the_texts_alone_are_ambiguous() {
    // Typing a "b" into "abb" after the "a": the difference between "abbb" and "abb" could be
    // any of the b's; only the record says it was the second byte.
    let mut e = TextEdit::new("abb").report_changes();
    e.set_caret(1);
    assert_eq!(edit(&mut e, |e| key(e, Key::Text("b".into()))), [change(1..1, 1)]);
    assert_eq!(edit(&mut e, TextEdit::undo), [change(1..2, 0)]);
    assert_eq!(edit(&mut e, TextEdit::redo), [change(1..1, 1)]);
}

#[test]
fn a_group_that_edits_two_places_undoes_as_two_changes_and_a_range_between_keeps_its_place() {
    let mut e = TextEdit::new("first middle last").report_changes();
    let forward = edit(&mut e, |e| e.undo_group(|e| {
        e.set_caret(0);
        e.insert("1 ");
        e.set_caret(e.text.len());
        e.insert("!");
    }));
    assert_eq!(forward, [change(0..0, 2), change(19..19, 1)]);
    let back = edit(&mut e, TextEdit::undo);
    assert_eq!(back, [change(19..20, 0), change(0..2, 0)], "the inverses, last first");
    // "middle" at 8..14 before the undo is at 6..12 after it, not swallowed by one wide change.
    let shift = |at: usize| back.iter().fold(at, |at, c| if at >= c.range.end { at - c.range.len() + c.inserted } else { at });
    assert_eq!((shift(8), shift(14)), (6, 12));
    assert_eq!(&e.text[6..12], "middle");
    assert_eq!(edit(&mut e, TextEdit::redo), forward);
}

#[test]
fn paste_cut_an_input_method_commit_and_replace_text_report() {
    let mut e = TextEdit::new("one two").report_changes();
    e.set_selection(4, 7);
    assert_eq!(edit(&mut e, |e| { e.paste("three"); }), [change(4..7, 5)]);
    e.set_selection(0, 4);
    assert_eq!(edit(&mut e, |e| { e.key(&command('x'), None); }), [change(0..4, 0)]);
    e.set_caret(e.text.len());
    assert!(edit(&mut e, |e| { e.ime(&Ime::Preedit("にほん".into(), None)); }).is_empty(), "a composition isn't in the text");
    assert_eq!(edit(&mut e, |e| { e.ime(&Ime::Commit("日本".into())); }), [change(5..5, 6)]);
    assert_eq!(e.text, "three日本");
    assert_eq!(edit(&mut e, |e| e.replace_text("new draft")), [change(0..11, 9)]);
    assert!(edit(&mut e, |e| e.replace_text("new draft")).is_empty(), "the same text changes nothing");
    assert!(edit(&mut e, TextEdit::undo).is_empty(), "replace_text dropped the history");
    assert_eq!(edit(&mut e, TextEdit::clear), [change(0..9, 0)]);
}

#[test]
fn off_by_default_and_steps_from_before_it_was_on_report_their_difference() {
    let mut e = TextEdit::new("cat");
    e.set_caret(3);
    key(&mut e, Key::Text("s".into()));
    assert!(e.take_changes().is_empty(), "no record without report_changes");
    e.set_report_changes(true);
    assert_eq!(edit(&mut e, TextEdit::undo), [change(3..4, 0)], "the texts' difference");
    assert_eq!(edit(&mut e, TextEdit::redo), [change(3..3, 1)]);
    key(&mut e, Key::Text("!".into()));
    e.set_report_changes(false);
    assert!(e.take_changes().is_empty(), "turning it off drops what wasn't taken");
}

#[test]
fn a_secure_field_reports_where_it_changed() {
    let mut e = TextEdit::new("").report_changes();
    e.set_secure(true);
    assert_eq!(edit(&mut e, |e| for c in ["p", "w"] { key(e, Key::Text(c.into())) }), [change(0..0, 1), change(1..1, 1)]);
    assert_eq!(edit(&mut e, |e| key(e, Key::Backspace)), [change(1..2, 0)]);
}

#[test]
fn copies_share_the_record_until_one_changes_and_never_compare_unequal_for_it() {
    let mut e = TextEdit::new("a").report_changes();
    e.set_caret(1);
    key(&mut e, Key::Text("b".into()));
    let mut copy = e.clone();
    assert!(copy == e, "the record isn't part of equality");
    assert_eq!(copy.take_changes(), [change(1..1, 1)]);
    assert_eq!(e.take_changes(), [change(1..1, 1)], "a copy taking its record leaves the original's");
}

#[test]
fn a_fields_on_input_change_reads_every_edit_the_user_makes() {
    struct Doc { edit: TextEdit, log: Vec<TextChange> }
    fn view(s: &Doc, _: &mut Cx<Doc>) -> El {
        text_input(&s.edit, TextStyle::new(14.0, 400.0)).key("doc").w(300.0).on_edit(|s: &mut Doc| &mut s.edit)
            .on_input_change(|s: &mut Doc, _| { let changes = s.edit.take_changes(); s.log.extend(changes); })
    }
    let mut edit = TextEdit::new("draft").report_changes();
    edit.focused = true;
    edit.set_caret(5);
    let mut rt = Runtime::new(Doc { edit, log: Vec::new() }, view, (320.0, 60.0), Env::new(false, 1.0));
    rt.frame(0.0);
    let mut t = 0.0;
    let mut step = |rt: &mut Runtime<Doc>| { t += 1.0 / 60.0; rt.frame(t); std::mem::take(&mut rt.state.log) };
    rt.key(KeyEvent { key: Key::Text("s".into()), mods: Mods::default() });
    assert_eq!(step(&mut rt), [change(5..5, 1)]);
    rt.paste(" two");
    assert_eq!(step(&mut rt), [change(6..6, 4)]);
    rt.ime(Ime::Preedit("か".into(), None));
    assert!(step(&mut rt).is_empty());
    rt.ime(Ime::Commit("火".into()));
    assert_eq!(step(&mut rt), [change(10..10, 3)]);
    rt.key(command('z'));
    assert_eq!(step(&mut rt), [change(10..13, 0)], "undo, through the field's own key");
    rt.key(command('z'));
    assert_eq!(step(&mut rt), [change(6..10, 0)]);
    assert_eq!(rt.state.edit.text, "drafts");
}
