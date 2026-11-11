use crate::a11y::AccessibilityLink;
use crate::{Cx, El, Env, Runtime, Tx, el, text};

#[derive(Default)]
struct Footer { saves: u32 }

fn view(s: &Footer, _: &mut Cx<Footer>) -> El {
    el().key("save").size(80.0, 20.0).child(text(format!("Saved {}", s.saves))).on_press_tx(|s: &mut Footer, tx: &mut Tx| {
        s.saves += 1;
        tx.announce("Saved");
    })
}

fn press(rt: &mut Runtime<Footer>, t: f64) {
    rt.pointer_down(10.0, 10.0);
    rt.pointer_up(10.0, 10.0);
    rt.frame(t);
}

/// The announcement nodes an update sends: (id, label, live).
fn announced(update: &accesskit::TreeUpdate) -> Vec<(u64, String, Option<accesskit::Live>)> {
    update.nodes.iter().filter(|(_, n)| n.live().is_some()).map(|(id, n)| (id.0, n.label().unwrap_or_default().to_owned(), n.live())).collect()
}

#[test]
fn an_announcement_is_a_new_polite_live_label_under_the_root_each_time() {
    let mut rt = Runtime::new(Footer::default(), view, (200.0, 100.0), Env::new(true, 1.0));
    rt.trace_channels = Some(Default::default());
    rt.frame(0.0);
    let mut link = AccessibilityLink::new();
    link.activate();
    let first = link.service(&mut rt, "Footer").expect("the whole tree");
    assert!(announced(&first).is_empty() && rt.announcement().is_none());

    press(&mut rt, 1.0 / 60.0);
    assert_eq!(rt.announcement(), Some("Saved"));
    let update = link.service(&mut rt, "Footer").expect("the announcement changes the tree");
    let nodes = announced(&update);
    assert_eq!(nodes.len(), 1);
    let (id, label, live) = nodes[0].clone();
    assert_eq!((label.as_str(), live), ("Saved", Some(accesskit::Live::Polite)));
    let root = update.nodes.iter().find(|(i, _)| *i == crate::runtime::a11y_tree::ROOT).expect("the root lists it").1.clone();
    assert!(root.children().iter().any(|c| c.0 == id));

    // The same text again is another node, so readers hear it again.
    press(&mut rt, 2.0 / 60.0);
    let again = announced(&link.service(&mut rt, "Footer").expect("updated"));
    assert_eq!(again.len(), 1);
    assert_ne!(again[0].0, id, "a new node, which adapters announce on adding");
    assert_eq!(again[0].1, "Saved");
    assert!(link.service(&mut rt, "Footer").is_none(), "nothing more to send");
    assert_eq!(rt.events.iter().filter(|e| *e == "announce").count(), 2, "{:?}", rt.events);

    let json = rt.accessibility_json().to_string();
    assert!(json.contains("live-polite") && json.contains("Saved"), "--tree shows it: {json}");
}

#[test]
fn the_last_announcement_of_a_turn_wins_and_empty_text_is_ignored() {
    let mut rt = Runtime::new(Footer::default(), view, (200.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|_, tx| {
        tx.announce("Bold");
        tx.announce("Italic");
        tx.announce("");
    });
    assert_eq!(rt.announcement(), Some("Italic"));
}

#[test]
fn a_reader_that_starts_later_does_not_hear_old_announcements() {
    let mut rt = Runtime::new(Footer::default(), view, (200.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    press(&mut rt, 1.0 / 60.0);
    assert_eq!(rt.announcement(), Some("Saved"), "kept for --tree with no reader");
    let mut link = AccessibilityLink::new();
    link.activate();
    let first = link.service(&mut rt, "Footer").expect("the whole tree");
    assert!(announced(&first).is_empty(), "activation drops it");
    assert_eq!(rt.announcement(), None);
}
