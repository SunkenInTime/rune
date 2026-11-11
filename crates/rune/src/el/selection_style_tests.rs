//! A field's selection while it doesn't have the keyboard (`SelectionStyle::unfocused`).

use crate::render::KIND_RECT;
use crate::{Color, Cx, El, Env, Runtime, SelectionStyle, TextEdit, TextStyle, col, text_input};

const ACTIVE: Color = Color { r: 0.0, g: 0.3, b: 0.9, a: 1.0 };
const INACTIVE: Color = Color { r: 0.7, g: 0.7, b: 0.7, a: 1.0 };
const UNFOCUSED: Color = Color { r: 0.85, g: 0.85, b: 0.85, a: 1.0 };
const INK: Color = Color { r: 0.2, g: 0.2, b: 0.2, a: 1.0 };

struct Form { note: TextEdit, search: TextEdit, unfocused: bool }

fn view(s: &Form, _: &mut Cx<Form>) -> El {
    let mut style = SelectionStyle::new(ACTIVE).inactive(INACTIVE, None);
    if s.unfocused { style = style.unfocused(UNFOCUSED, Some(INK)); }
    col().child(text_input(&s.note, TextStyle::new(14.0, 400.0)).key("note").w(200.0).on_edit(|s: &mut Form| &mut s.note).selection_style(style))
        .child(text_input(&s.search, TextStyle::new(14.0, 400.0)).key("search").w(200.0).on_edit(|s: &mut Form| &mut s.search))
}

fn painted(rt: &Runtime<Form>, c: Color) -> usize {
    rt.instances.iter().filter(|i| i.params[1] == KIND_RECT && i.fill == c.premul(1.0)).count()
}

fn form(unfocused: bool) -> Runtime<Form> {
    let mut note = TextEdit::new("keep this selected");
    note.focused = true;
    note.set_selection(5, 9);
    let mut rt = Runtime::new(Form { note, search: TextEdit::new(""), unfocused }, view, (240.0, 80.0), Env::new(false, 1.0));
    rt.window_focus(true);
    rt.frame(0.0);
    rt
}

fn focus_search(rt: &mut Runtime<Form>) {
    rt.update(|s, _| s.search.focused = true);
    rt.frame(0.01);
}

#[test]
fn an_unfocused_field_draws_its_selection_in_the_unfocused_pair_and_takes_back_the_active_one() {
    let mut rt = form(true);
    assert_eq!((painted(&rt, ACTIVE), painted(&rt, UNFOCUSED)), (1, 0), "focused: the active pair");
    focus_search(&mut rt);
    assert!(!rt.state.note.focused && rt.state.note.range() == (5, 9), "the note kept its selection");
    assert_eq!((painted(&rt, ACTIVE), painted(&rt, UNFOCUSED)), (0, 1), "another field has the keyboard: the unfocused pair");
    let glyph = |rt: &Runtime<Form>| rt.instances.iter().any(|i| i.params[1] != KIND_RECT && i.fill == INK.premul(1.0));
    assert!(glyph(&rt), "the selected glyphs take its ink");

    rt.window_focus(false);
    rt.frame(0.02);
    assert_eq!((painted(&rt, INACTIVE), painted(&rt, UNFOCUSED)), (0, 1), "an inactive window keeps the unfocused pair for a field without the keyboard");
    rt.window_focus(true);

    rt.update(|s, _| { s.search.focused = false; s.note.focused = true; });
    rt.frame(0.03);
    assert_eq!((painted(&rt, ACTIVE), painted(&rt, UNFOCUSED)), (1, 0), "focused again: the active pair");
}

#[test]
fn without_an_unfocused_pair_an_unfocused_field_draws_no_selection_as_before() {
    let mut rt = form(false);
    assert_eq!(painted(&rt, ACTIVE), 1);
    focus_search(&mut rt);
    assert_eq!((painted(&rt, ACTIVE), painted(&rt, INACTIVE), painted(&rt, UNFOCUSED)), (0, 0, 0));
}

#[test]
fn the_window_losing_focus_keeps_the_focused_fields_inactive_pair() {
    let mut rt = form(true);
    rt.window_focus(false);
    rt.frame(0.01);
    assert_eq!((painted(&rt, INACTIVE), painted(&rt, UNFOCUSED)), (1, 0), "the field still has the keyboard in its window: the inactive pair");
}
