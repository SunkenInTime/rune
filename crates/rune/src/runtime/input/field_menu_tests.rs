use crate::popup_menu::Entry;
use crate::{Cx, El, Env, Key, KeyEvent, Mods, PopupItem, PopupMenu, Runtime, TextEdit, TextStyle, col, text_input};

/// A writing app's field: Comment needs a selection, and every pick is logged with its offset.
struct Doc {
    edit: TextEdit,
    picks: Vec<(String, usize)>,
    built_at: Vec<usize>,
    items: bool,
}

fn view(s: &Doc, _: &mut Cx<Doc>) -> El {
    let field = text_input(&s.edit, TextStyle::new(14.0, 400.0)).key("field").size(300.0, 40.0).on_edit(|s: &mut Doc| &mut s.edit);
    let root = col().child(field);
    if !s.items { return root; }
    // On an ancestor, as an app wrapping a kit field sets it.
    root.input_menu(
        |s: &mut Doc, at| {
            s.built_at.push(at);
            let (a, b) = s.edit.range();
            PopupMenu::new()
                .item(PopupItem::new("comment", "Add Comment").enabled(a != b))
                .separator()
                .item(PopupItem::new("look-up", "Look Up").checked(true))
        },
        |s: &mut Doc, id, at, _| s.picks.push((id.to_owned(), at)),
    )
}

fn doc(items: bool) -> Runtime<Doc> {
    let edit = TextEdit::new("one, two three");
    let mut rt = Runtime::new(Doc { edit, picks: vec![], built_at: vec![], items }, view, (400.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt
}

fn right_click(rt: &mut Runtime<Doc>, x: f32) {
    rt.secondary_down(x, 10.0);
    rt.secondary_up(x, 10.0);
    rt.frame(1.0 / 60.0);
}

/// The menu's top-level ids, `-` for a separator.
fn ids(menu: &PopupMenu) -> Vec<String> {
    menu.entries.iter().map(|e| match e {
        Entry::Item(item) => item.id.clone(),
        Entry::Separator => "-".into(),
        Entry::Submenu(title, ..) => title.clone(),
    }).collect()
}

fn rune_items() -> Vec<&'static str> {
    if cfg!(target_os = "macos") { vec!["cut", "copy", "paste"] } else { vec!["cut", "copy", "paste", "select-all"] }
}

#[test]
fn the_apps_items_come_first_then_runes_own() {
    let mut rt = doc(true);
    right_click(&mut rt, 60.0);
    let menu = rt.popup_menu().expect("the field's menu is open").clone();
    let mut want: Vec<String> = ["comment", "-", "look-up", "-"].map(String::from).to_vec();
    want.extend(rune_items().into_iter().map(String::from));
    assert_eq!(ids(&menu), want);
    assert!(!menu.find("comment").unwrap().is_enabled(), "nothing selected: the app's Comment is greyed");
    assert!(menu.find("look-up").unwrap().is_checked());
    let at = rt.state.built_at[0];
    assert!(at > 0 && at < rt.state.edit.text.len(), "built for the offset under the click, not {at}");
    assert_eq!(rt.state.edit.range(), (at, at), "the click put the caret there first, as without the hook");

    rt.resolve_popup(Some("look-up")).unwrap();
    assert_eq!(rt.state.picks, [("look-up".to_owned(), at)], "the pick hears its id and the click's offset");
}

#[test]
fn a_click_in_the_selection_keeps_it_for_the_apps_items_and_runes() {
    let mut rt = doc(true);
    rt.update(|s, _| s.edit.set_selection(0, 8));
    rt.frame(1.0 / 60.0);
    right_click(&mut rt, 20.0);
    let menu = rt.popup_menu().unwrap().clone();
    assert_eq!(rt.state.edit.range(), (0, 8), "the selection stays");
    assert!(menu.find("comment").unwrap().is_enabled() && menu.find("copy").unwrap().is_enabled());
    let at = rt.state.built_at[0];
    assert!(at < 8);
    rt.resolve_popup(Some("comment")).unwrap();
    assert_eq!(rt.state.picks, [("comment".to_owned(), at)]);
}

#[test]
fn runes_items_still_act_beside_the_apps() {
    let mut rt = doc(true);
    rt.update(|s, _| s.edit.set_selection(0, 3));
    rt.frame(1.0 / 60.0);
    right_click(&mut rt, 10.0);
    rt.resolve_popup(Some("cut")).unwrap();
    rt.frame(2.0 / 60.0);
    assert_eq!(rt.state.edit.text, ", two three", "Cut cut");
    assert!(rt.state.picks.is_empty(), "and the app's handler heard nothing");
}

#[test]
fn the_keyboard_opens_it_for_the_caret() {
    let mut rt = doc(true);
    rt.update(|s, _| { s.edit.focused = true; s.edit.set_caret(5); });
    rt.frame(1.0 / 60.0);
    rt.key(KeyEvent { key: Key::ContextMenu, mods: Mods::default() });
    rt.frame(2.0 / 60.0);
    assert_eq!(ids(rt.popup_menu().expect("the Menu key opened it"))[0], "comment");
    assert_eq!(rt.state.built_at, [5]);
    rt.resolve_popup(Some("look-up")).unwrap();
    assert_eq!(rt.state.picks, [("look-up".to_owned(), 5)]);
}

#[test]
fn without_items_the_menu_is_as_it_was() {
    let mut rt = doc(false);
    right_click(&mut rt, 60.0);
    assert_eq!(ids(rt.popup_menu().unwrap()), rune_items());
}
