use super::*;
use crate::{Key, KeyEvent, Mark, MarkRange, Mods};

fn alt(key: Key) -> KeyEvent {
    KeyEvent { key, mods: Mods { alt: true, ..Mods::default() } }
}

/// The stops Option+Right makes from the start, then Option+Left from the end.
fn stops(text: &str, motion: WordMotion) -> (Vec<usize>, Vec<usize>) {
    let mut e = TextEdit::new(text).word_motion(motion);
    let walk = |e: &mut TextEdit, key: Key| {
        let mut seen = Vec::new();
        for _ in 0..64 {
            let before = e.focus;
            e.key(&alt(key.clone()), None);
            if e.focus == before { break; }
            seen.push(e.focus);
        }
        seen
    };
    e.set_caret(0);
    let forward = walk(&mut e, Key::Right);
    e.set_caret(text.len());
    let back = walk(&mut e, Key::Left);
    (forward, back)
}

#[test]
fn unicode_word_motion_stops_at_word_ends_forward_and_starts_back_passing_punctuation() {
    let u = WordMotion::Unicode;
    assert_eq!(stops("one, two", u), (vec![3, 8], vec![5, 0]));
    assert_eq!(stops("one, two", WordMotion::Whitespace), (vec![4, 8], vec![5, 0]), "the default keeps the comma with its word");
    assert_eq!(stops("don't stop", u), (vec![5, 10], vec![6, 0]), "an apostrophe inside a word is part of it");
    assert_eq!(stops("a.b c", u), (vec![3, 5], vec![4, 0]), "a full stop between letters joins them, as in a domain");
    assert_eq!(stops("end.  Next", u), (vec![3, 10], vec![6, 0]));
    assert_eq!(stops("(see: “this”)", u), (vec![4, 13, 17], vec![9, 1, 0]));
    assert_eq!(stops("x -- y", u), (vec![1, 6], vec![5, 0]));
    assert_eq!(stops("3.14 or 2,000", u), (vec![4, 7, 13], vec![8, 5, 0]), "numbers keep their separators");
    assert_eq!(stops("one\n\ntwo,", u), (vec![3, 8, 9], vec![5, 0]), "across blank lines");
    assert_eq!(stops("", u), (vec![], vec![]));
    assert_eq!(stops("  ,  ", u), (vec![5], vec![0]), "no words: to the ends");
}

#[test]
fn emoji_are_words_of_their_own_and_cjk_splits_into_dictionary_words() {
    let u = WordMotion::Unicode;
    // "hi 👋🏽 there": the waving hand with its skin tone is one stop, never split.
    let wave = "hi 👋🏽 there";
    assert_eq!(stops(wave, u), (vec![2, 11, 17], vec![12, 3, 0]));
    // 你好世界 is two words, 你好 and 世界; the full-width comma is passed over.
    let cjk = "你好世界，再见";
    assert_eq!(stops(cjk, u), (vec![6, 12, 21], vec![15, 6, 0]));
}

#[test]
fn option_backspace_and_delete_remove_to_the_unicode_stops() {
    let mut e = TextEdit::new("one, two").word_motion(WordMotion::Unicode);
    e.key(&alt(Key::Backspace), None);
    assert_eq!((e.text.as_str(), e.focus), ("one, ", 5), "back to the start of the word");
    e.key(&alt(Key::Backspace), None);
    assert_eq!(e.text, "", "the comma and space go with the word before them");
    e.undo();
    assert_eq!(e.text, "one, ");

    let mut e = TextEdit::new("one, two").word_motion(WordMotion::Unicode);
    e.set_caret(3);
    e.key(&alt(Key::Delete), None);
    assert_eq!((e.text.as_str(), e.focus), ("one", 3), "forward to the end of the next word");

    let mut e = TextEdit::new("one, two");
    e.key(&alt(Key::Backspace), None);
    e.key(&alt(Key::Backspace), None);
    assert_eq!(e.text, "", "the default deletes by white space");
    let mut e = TextEdit::new("one, two");
    e.set_caret(5);
    e.key(&alt(Key::Backspace), None);
    assert_eq!(e.text, "two", "the default takes the comma with its word");
}

#[test]
fn shift_selects_to_the_stops_and_a_pill_is_one_word() {
    let mut e = TextEdit::new("one, two").word_motion(WordMotion::Unicode);
    e.set_caret(0);
    e.key(&KeyEvent { key: Key::Right, mods: Mods { alt: true, shift: true, ..Mods::default() } }, None);
    assert_eq!(e.selected(), "one");

    // "see @ann.lee, ok" with "@ann.lee" a pill.
    let mut e = TextEdit::new("see @ann.lee, ok").rich().word_motion(WordMotion::Unicode);
    e.set_marks(vec![MarkRange { start: 4, end: 12, mark: Mark::Pill("user:1".into()) }]);
    let (forward, back) = {
        let mut seen = (Vec::new(), Vec::new());
        e.set_caret(0);
        for _ in 0..4 { e.key(&alt(Key::Right), None); seen.0.push(e.focus); }
        e.set_caret(e.text.len());
        for _ in 0..4 { e.key(&alt(Key::Left), None); seen.1.push(e.focus); }
        seen
    };
    assert_eq!(forward, [3, 12, 16, 16]);
    assert_eq!(back, [14, 4, 0, 0]);
    e.set_caret(12);
    e.key(&alt(Key::Backspace), None);
    assert_eq!(e.text, "see , ok", "the pill goes whole");
}

#[test]
fn a_hidden_secret_still_moves_to_its_ends_and_the_setting_survives_replacing_text() {
    let mut e = TextEdit::new("pass, word").secure().word_motion(WordMotion::Unicode);
    e.set_caret(0);
    e.key(&alt(Key::Right), None);
    assert_eq!(e.focus, e.text.len());

    let mut e = TextEdit::new("").word_motion(WordMotion::Unicode);
    e.replace_text("one, two");
    e.set_caret(0);
    e.key(&alt(Key::Right), None);
    assert_eq!(e.focus, 3);
    e.set_word_motion(WordMotion::Whitespace);
    e.set_caret(0);
    e.key(&alt(Key::Right), None);
    assert_eq!(e.focus, 4);
}
