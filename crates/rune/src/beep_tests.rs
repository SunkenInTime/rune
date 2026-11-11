use crate::{Cx, El, Env, Key, KeyEvent, Mods, Runtime, Tx, el};

#[derive(Default)]
struct Refuse { refused: u32 }

fn view(_: &Refuse, _: &mut Cx<Refuse>) -> El {
    el().key("button").size(80.0, 20.0).on_press_tx(|s: &mut Refuse, tx: &mut Tx| {
        s.refused += 1;
        tx.beep();
        tx.beep();
    })
}

#[test]
fn a_headless_beep_is_counted_once_a_turn_and_traced_not_played() {
    let mut rt = Runtime::new(Refuse::default(), view, (200.0, 100.0), Env::new(true, 1.0));
    rt.trace_channels = Some(Default::default());
    rt.frame(0.0);
    assert_eq!(rt.beeps(), 0);
    rt.pointer_down(10.0, 10.0);
    rt.pointer_up(10.0, 10.0);
    rt.frame(1.0 / 60.0);
    assert_eq!((rt.state.refused, rt.beeps()), (1, 1), "two beeps in one turn are one");
    rt.update(|_, tx| tx.beep());
    assert_eq!(rt.beeps(), 2, "an update beeps without a frame");
    rt.frame(2.0 / 60.0);
    assert_eq!(rt.beeps(), 2, "a frame with nothing asked stays quiet");
    assert!(!rt.open_urls_natively, "a test never plays the sound");
    assert_eq!(rt.events.iter().filter(|e| *e == "beep").count(), 2, "--trace records each: {:?}", rt.events);
}

#[test]
fn a_key_handler_beeps() {
    fn keyed(_: &Refuse, _: &mut Cx<Refuse>) -> El {
        el().key("keys").size(80.0, 20.0).focusable(true).on_key(|_: &mut Refuse, ev: &KeyEvent, tx: &mut Tx| {
            if ev.key == Key::Escape { tx.beep(); true } else { false }
        })
    }
    let mut rt = Runtime::new(Refuse::default(), keyed, (200.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let (x, y) = rt.find("keys").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.5 / 60.0);
    rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.beeps(), 1);
}
