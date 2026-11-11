//! Where a field's word motion stops: at white space, Rune's default, or at Unicode word
//! boundaries as AppKit's text views stop (`moveWordForward:`, `deleteWordBackward:`).

use super::TextEdit;

/// Where Option+Left/Right (Ctrl off macOS) stop and what Option+Backspace/Delete remove.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum WordMotion {
    /// At runs of white space: "one, two" has stops before "one", after "one,", before "two"
    /// and after "two". Punctuation travels with the word it touches.
    #[default]
    Whitespace,
    /// At Unicode word boundaries (UAX #29, with dictionaries for Chinese, Japanese, Thai, Lao,
    /// Khmer and Burmese), as AppKit's text views do: forward goes to the end of the next word,
    /// back to the start of the previous one, and punctuation and white space between words are
    /// passed over. "one, two" stops after "one" and after "two" going forward, before "two"
    /// and before "one" going back; "don't" and "a.b" are one word each.
    Unicode,
}

/// The word segments of `text` that count as words, as byte ranges in order: letters, numbers
/// and ideographs (ICU's word-like segments), and segments of other symbols such as emoji.
/// White space and punctuation between them are not words.
fn words(text: &str) -> impl Iterator<Item = (usize, usize)> + '_ {
    use icu_segmenter::{WordSegmenter, options::WordBreakInvariantOptions};
    let mut it = WordSegmenter::new_dictionary(WordBreakInvariantOptions::default()).segment_str(text);
    let mut start = 0;
    std::iter::from_fn(move || {
        loop {
            let end = it.next()?;
            if end == 0 {
                continue;
            }
            let (a, word_like) = (start, it.is_word_like());
            start = end;
            if word_like || text[a..end].chars().any(is_symbol) {
                return Some((a, end));
            }
        }
    })
}

/// Not white space, punctuation or a joiner: an emoji or other symbol, which AppKit treats as a
/// word of its own.
fn is_symbol(c: char) -> bool {
    !(c.is_whitespace() || c.is_control() || c.is_ascii_punctuation() || is_punctuation(c) || matches!(c, '\u{200b}'..='\u{200d}' | '\u{fe00}'..='\u{fe0f}'))
}

/// Punctuation outside ASCII: general punctuation, CJK and full-width punctuation, Latin-1's
/// quotes and marks, and the other scripts' sentence marks a writer meets.
fn is_punctuation(c: char) -> bool {
    matches!(c,
        '\u{a1}' | '\u{a7}' | '\u{ab}' | '\u{b6}' | '\u{b7}' | '\u{bb}' | '\u{bf}'
        | '\u{37e}' | '\u{387}' | '\u{55a}'..='\u{55f}' | '\u{589}' | '\u{5be}' | '\u{5c0}' | '\u{5c3}' | '\u{5f3}' | '\u{5f4}'
        | '\u{60c}' | '\u{60d}' | '\u{61b}' | '\u{61f}' | '\u{66a}'..='\u{66d}' | '\u{6d4}' | '\u{964}' | '\u{965}'
        | '\u{2010}'..='\u{2027}' | '\u{2030}'..='\u{205e}' | '\u{2e00}'..='\u{2e4f}'
        | '\u{3001}'..='\u{3003}' | '\u{3008}'..='\u{3011}' | '\u{3014}'..='\u{301f}' | '\u{30fb}'
        | '\u{fe10}'..='\u{fe19}' | '\u{fe30}'..='\u{fe4f}' | '\u{fe50}'..='\u{fe6b}'
        | '\u{ff01}'..='\u{ff0f}' | '\u{ff1a}'..='\u{ff20}' | '\u{ff3b}'..='\u{ff40}' | '\u{ff5b}'..='\u{ff65}')
}

impl TextEdit {
    /// How this field's word motion and word deletion stop: at white space (the default) or at
    /// Unicode word boundaries as AppKit's do ([`WordMotion::Unicode`]). Applies to Option+Left
    /// and Option+Right (Ctrl off macOS), with Shift to select, and Option+Backspace and
    /// Option+Delete. A pill is one word either way, and a hidden secure field still moves to
    /// its ends. In text holding right-to-left runs the arrows move across the line as drawn,
    /// by the text layout's own word stops, whichever this is.
    ///
    /// ```
    /// # use rune::{TextEdit, WordMotion};
    /// let e = TextEdit::new("one, two").word_motion(WordMotion::Unicode);
    /// ```
    pub fn word_motion(mut self, motion: WordMotion) -> Self {
        self.word_motion = motion;
        self
    }

    /// Sets [`word_motion`](Self::word_motion) on a field the app already holds.
    pub fn set_word_motion(&mut self, motion: WordMotion) {
        self.word_motion = motion;
    }

    /// The previous Unicode word stop before `i`: the start of the word `i` is inside or after.
    /// A pill is a word.
    pub(super) fn prev_unicode_word(&self, i: usize) -> usize {
        let s = &self.text;
        let i = s.floor_char_boundary(i.min(s.len()));
        let pill = self.pills().map(|(r, _)| r.start).filter(|&a| a < i).max();
        // Word boundaries always break at a line break, so each line segments on its own.
        let mut end = i;
        let word = loop {
            let line = s[..end].rfind('\n').map_or(0, |n| n + 1);
            let found = words(&s[line..]).map(|(a, b)| (line + a, line + b)).take_while(|&(a, _)| a < i)
                .filter(|&(a, b)| !self.inside_pill(a, b)).last();
            if found.is_some() || line == 0 { break found.map(|(a, _)| a); }
            end = line - 1;
        };
        word.max(pill).unwrap_or(0)
    }

    /// The next Unicode word stop after `i`: the end of the word `i` is inside or before.
    pub(super) fn next_unicode_word(&self, i: usize) -> usize {
        let s = &self.text;
        let i = s.floor_char_boundary(i.min(s.len()));
        let pill = self.pills().map(|(r, _)| r.end).filter(|&b| b > i).min();
        let line = s[..i].rfind('\n').map_or(0, |n| n + 1);
        let word = words(&s[line..]).map(|(a, b)| (line + a, line + b)).find(|&(a, b)| b > i && !self.inside_pill(a, b)).map(|(_, b)| b);
        match (word, pill) { (Some(w), Some(p)) => w.min(p), (w, p) => w.or(p).unwrap_or(s.len()) }
    }

    /// A word segment that is part of a pill's label, which moves as one with its pill.
    fn inside_pill(&self, a: usize, b: usize) -> bool {
        self.pills().any(|(r, _)| r.start < b && a < r.end)
    }
}

#[cfg(test)]
#[path = "words_tests.rs"]
mod tests;
