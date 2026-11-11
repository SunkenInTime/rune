//! A long text of many paragraphs laid out a paragraph at a time: each paragraph (or run of
//! paragraphs that must shape together) is shaped and broken on its own, cached by its text,
//! spans and pills, and the parts are joined into one `parley::Layout` (`Layout::join_part`),
//! so an edit shapes and breaks only the paragraph it touched and a span that moves only within
//! one paragraph (a sentence highlight) reshapes only that one.
//!
//! The joined layout is the layout of the whole text, glyph for glyph and line for line, where
//! nothing in shaping crosses a newline. Parley shapes a whole text as one bidi paragraph and
//! carries the script of neutral characters across newlines, so a text with right-to-left or
//! embedding characters (or a line break other than `\n`) shapes whole, and two paragraphs are
//! shaped apart only when the second starts as it would after the first: with a Latin letter, or
//! with a neutral character after Latin text and before more of it. What the whole text decides
//! for every paragraph (ligatures under `trak` correction, the inline code strut, the `trak`
//! check's second shaping) is decided over the whole text and passed to each paragraph.

use std::hash::Hasher;
use std::ops::Range;

use icu_properties::props::{BidiClass, Script};
use icu_properties::CodePointMapData;

use super::hash::{KeyHasher, run_hash};
use super::layout_cache::{Cache as EntryCache, evict};
use super::shape::WholeFacts;
use super::synthetic_bold::SYNTHETIC_BOLD;
use super::*;

/// Texts shorter than this shape whole: joining their paragraphs saves nothing.
pub(super) const MIN_BYTES: usize = 2048;

/// Paragraph facts kept before the cache starts over.
const MAX_FACTS: usize = 1 << 14;

/// A text [`TextSystem::layout_keyed`] lays out, as it was asked to.
pub(super) struct Whole<'a> {
    pub(super) text: &'a str,
    pub(super) spans: &'a [Span],
    pub(super) blocks: &'a [crate::edit::Block],
    pub(super) pills: &'a [PillBox],
    pub(super) style: TextStyle,
    pub(super) scale: f32,
    pub(super) max_width: Option<f32>,
    pub(super) align: Alignment,
    pub(super) wrap_anywhere: bool,
    pub(super) base: parley::BaseDirection,
    /// Whether lines break by blocks and pills (`break_paragraphs`).
    pub(super) blocks_path: bool,
    pub(super) shape_key: u64,
    /// Its `layouts` key.
    pub(super) key: u64,
}

/// The paragraph layouts joined texts are made of.
#[derive(Default)]
pub(crate) struct Paragraphs {
    /// Each part's shaped and broken layout, by its text, spans, pills, style and scale.
    parts: EntryCache<Part>,
    /// What each paragraph's text holds, by its hash.
    facts: crate::runtime::HashMap<u64, Facts>,
    /// The parts each joined layout in `layouts` is made of, by its key, kept while it is.
    joined: crate::runtime::HashMap<u64, Rc<[u64]>>,
    /// Joined layouts handed out lately, whose buffers the next join reuses once nothing else
    /// holds them (typing replaces one each keystroke).
    spare: Vec<Rc<Layout<Ink>>>,
    /// Texts at least this long are joined from paragraphs ([`MIN_BYTES`]; tests lower it).
    pub(super) min_bytes: usize,
}

/// Joined layouts [`Paragraphs::spare`] keeps.
const SPARE: usize = 3;

impl Paragraphs {
    pub(super) fn new() -> Self {
        Paragraphs { min_bytes: MIN_BYTES, ..Default::default() }
    }

    pub(super) fn clear(&mut self) {
        self.parts.clear();
        self.facts.clear();
        self.joined.clear();
        self.spare.clear();
    }

    /// An empty layout, with the buffers of a joined one no one holds any more when there is one.
    fn reuse(&mut self) -> Layout<Ink> {
        match self.spare.iter().position(|l| Rc::strong_count(l) == 1) {
            Some(i) => Rc::try_unwrap(self.spare.swap_remove(i)).unwrap_or_default(),
            None => Layout::new(),
        }
    }
}

/// One part of a joined text: a paragraph, or paragraphs that shape together.
pub(super) struct Part {
    layout: Layout<Ink>,
    probe: Probe,
    /// The shaping it holds: `None` the first, `Some(nonzero)` the one with each run's own
    /// spacing ([`Exact::Force`]).
    forced: Option<bool>,
    /// The width, blocks and top its lines were broken at, as bits and a hash.
    broken: (u32, u64, u64),
}

/// How a paragraph's first character starts it, for whether it can shape apart from the one
/// before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lead {
    /// A Latin letter: it starts a Latin run whatever came before.
    Latin,
    /// A character of no script (punctuation, digits, emoji, the newline of an empty
    /// paragraph), which takes the script before it.
    Neutral,
    /// Anything else: another script, a combining mark, an unassigned character.
    Other,
}

/// What a paragraph's text holds.
#[derive(Clone, Copy, Debug)]
struct Facts {
    /// A character that resolves right to left or embeds a direction, or a line break other
    /// than `\n`: the text shapes whole.
    whole_only: bool,
    lead: Lead,
    /// Its first and last character of a real script: `Some(true)` for Latin.
    first_real: Option<bool>,
    last_real: Option<bool>,
    chars: u32,
}

fn facts_of(p: &str) -> Facts {
    let mut f = Facts { whole_only: false, lead: Lead::Neutral, first_real: None, last_real: None, chars: 0 };
    let scripts = CodePointMapData::<Script>::new();
    let classes = CodePointMapData::<BidiClass>::new();
    for (i, c) in p.chars().enumerate() {
        let real = if c.is_ascii() {
            f.whole_only |= matches!(c, '\r' | '\u{b}' | '\u{c}');
            c.is_ascii_alphabetic().then_some(true)
        } else {
            f.whole_only |= matches!(c, '\u{85}' | '\u{2028}' | '\u{2029}')
                || matches!(
                    classes.get(c),
                    BidiClass::RightToLeft
                        | BidiClass::ArabicLetter
                        | BidiClass::ArabicNumber
                        | BidiClass::LeftToRightEmbedding
                        | BidiClass::RightToLeftEmbedding
                        | BidiClass::LeftToRightOverride
                        | BidiClass::RightToLeftOverride
                        | BidiClass::PopDirectionalFormat
                        | BidiClass::LeftToRightIsolate
                        | BidiClass::RightToLeftIsolate
                        | BidiClass::FirstStrongIsolate
                        | BidiClass::PopDirectionalIsolate
                );
            match scripts.get(c) {
                Script::Common => None,
                Script::Latin => Some(true),
                // Inherited (a mark) and Unknown take no script of their own, but a paragraph
                // can't start with one apart from the text before it.
                Script::Inherited | Script::Unknown => {
                    if i == 0 {
                        f.lead = Lead::Other;
                    }
                    None
                }
                _ => Some(false),
            }
        };
        if i == 0 && f.lead != Lead::Other {
            f.lead = match real {
                Some(true) => Lead::Latin,
                Some(false) => Lead::Other,
                None => Lead::Neutral,
            };
        }
        if real.is_some() {
            f.first_real = f.first_real.or(real);
            f.last_real = real;
        }
        f.chars += 1;
    }
    f
}

/// The paragraphs of `text`, each ending after its `\n` but the last (none after a final `\n`).
fn paragraph_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0];
    starts.extend(text.match_indices('\n').map(|(i, _)| i + 1).filter(|&s| s < text.len()));
    starts
}

/// Which paragraphs start a part: those that shape the same apart from the text before them.
/// None when the text must shape whole.
fn cuts(facts: &[Facts]) -> Option<Vec<bool>> {
    if facts.iter().any(|f| f.whole_only) {
        return None;
    }
    let latin = |x: Option<bool>| x.unwrap_or(true);
    // The first real script from each paragraph on.
    let mut next = vec![None; facts.len() + 1];
    for k in (0..facts.len()).rev() {
        next[k] = facts[k].first_real.or(next[k + 1]);
    }
    // Neutral characters at the text's start take its first real script; a part alone takes
    // its own, or Latin when it has none.
    if facts[0].lead == Lead::Neutral && !latin(next[0]) {
        return None;
    }
    let mut out = vec![true; facts.len()];
    let mut before = None;
    for (k, f) in facts.iter().enumerate() {
        if k > 0 {
            out[k] = match f.lead {
                Lead::Latin => true,
                // Neutral characters take the script before them (or the text's first);
                // shaped apart, the part's first. Both must be Latin.
                Lead::Neutral => latin(before.or(next[0])) && latin(next[k]),
                Lead::Other => false,
            };
        }
        before = f.last_real.or(before);
    }
    Some(out)
}

/// `blocks` from paragraph `k` on, as `break_paragraphs` reads them for a part starting there.
fn blocks_from(blocks: &[crate::edit::Block], k: usize) -> &[crate::edit::Block] {
    &blocks[k.min(blocks.len())..]
}

/// The room under paragraph `k`'s last line, in device px, as `break_paragraphs` adds it.
fn after(blocks: &[crate::edit::Block], k: usize, scale: f32) -> f64 {
    let blk = blocks.get(k).copied().unwrap_or(crate::edit::Block::Paragraph);
    (blk.space_after_pt() * scale).round() as f64
}

/// One part of the text being joined: its bytes and paragraphs, and its spans and pills with
/// their places in the whole text's lists.
struct Piece {
    bytes: Range<usize>,
    paragraphs: Range<usize>,
    chars: usize,
    spans: Vec<Span>,
    span_of: Vec<u32>,
    pills: Vec<PillBox>,
    pill_of: Vec<usize>,
}

impl TextSystem {
    /// [`Self::layout_keyed`]'s layout of `w` joined from its paragraphs, or None when it must
    /// shape whole: short, unwrapped, right to left, or one part.
    pub(super) fn layout_paragraphs(&mut self, w: &Whole<'_>) -> Option<Rc<Layout<Ink>>> {
        let max_width = w.max_width?;
        if w.text.len() < self.paragraphs.min_bytes || w.base == parley::BaseDirection::Rtl || !w.text.contains('\n') {
            return None;
        }
        let pieces = self.pieces(w)?;
        let _time = crate::frame::profile::Scope::new(crate::frame::profile::Phase::Text);
        let whole = self.whole_facts(w.text, w.spans, w.style, w.scale);
        let keys: Vec<u64> = pieces.iter().map(|p| part_key(w, p, whole)).collect();
        let g = self.generation;
        let (mut shaped, mut rebroken) = (false, false);
        let block_keys: Vec<u64> = pieces
            .iter()
            .map(|p| {
                let mut h = KeyHasher::default();
                (w.blocks_path, &w.blocks[p.paragraphs.start.min(w.blocks.len())..(p.paragraphs.end + 1).min(w.blocks.len())]).hash(&mut h);
                h.finish()
            })
            .collect();
        // Every part shaped once, to learn whether any run's font needs other spacing than its
        // style predicted; a whole text then shapes again with each run's own spacing.
        for ((p, &key), &bk) in pieces.iter().zip(&keys).zip(&block_keys) {
            if !self.paragraphs.parts.contains_key(&key) {
                shaped = true;
                let blocks = blocks_from(w.blocks, p.paragraphs.start);
                let (blocks_path, size, scale) = (w.blocks_path, w.style.size, w.scale);
                let break_lines = |l: &mut Layout<Ink>| break_paragraphs(l, blocks_path, blocks, size, scale, Some(max_width), 0.0);
                let text = &w.text[p.bytes.clone()];
                let (layout, probe) = self.shape_text(text, &p.spans, &p.pills, w.style, w.scale, w.wrap_anywhere, w.base, &break_lines, Some(whole), Exact::Probe);
                let part = Part { layout, probe, forced: None, broken: (max_width.to_bits(), bk, 0.0f64.to_bits()) };
                self.paragraphs.parts.insert(key, layout_cache::Entry::new(part, g, text.len(), 0));
            }
        }
        let (mismatch, nonzero) = keys
            .iter()
            .filter_map(|k| self.paragraphs.parts.get(k).map(|e| e.value.probe))
            .fold((false, false), |(m, n), p| (m || p.mismatch, n || p.exact_nonzero));
        let forced = mismatch.then_some(nonzero);
        // Then each broken where the parts before it end. A part whose text, spans and pills
        // come twice (empty paragraphs) shares one layout, but one broken elsewhere than its
        // first time (another block, another top) is a copy.
        let mut tops = Vec::with_capacity(pieces.len());
        let mut copies: Vec<Option<Layout<Ink>>> = Vec::with_capacity(pieces.len());
        let mut seen: crate::runtime::HashMap<u64, (u32, u64, u64)> = Default::default();
        let mut y = 0.0f64;
        for ((p, &key), &bk) in pieces.iter().zip(&keys).zip(&block_keys) {
            tops.push(y);
            let top = y.fract();
            let blocks = blocks_from(w.blocks, p.paragraphs.start);
            let broken = (max_width.to_bits(), bk, top.to_bits());
            let (blocks_path, size, scale) = (w.blocks_path, w.style.size, w.scale);
            let break_lines = |l: &mut Layout<Ink>| break_paragraphs(l, blocks_path, blocks, size, scale, Some(max_width), top);
            if self.paragraphs.parts.get(&key)?.value.forced != forced {
                shaped = true;
                let text = &w.text[p.bytes.clone()];
                let mode = forced.map_or(Exact::Probe, |nonzero| Exact::Force { nonzero });
                let (layout, probe) = self.shape_text(text, &p.spans, &p.pills, w.style, w.scale, w.wrap_anywhere, w.base, &break_lines, Some(whole), mode);
                self.paragraphs.parts.insert(key, layout_cache::Entry::new(Part { layout, probe, forced, broken }, g, text.len(), 0));
            }
            let e = self.paragraphs.parts.get_mut(&key)?;
            e.used = g;
            let copy = match seen.get(&key) {
                Some(&first) if first != broken => {
                    let mut l = e.value.layout.clone();
                    break_lines(&mut l);
                    Some(l)
                }
                Some(_) => None,
                None => {
                    seen.insert(key, broken);
                    if e.value.broken != broken {
                        rebroken = true;
                        break_lines(&mut e.value.layout);
                        e.value.broken = broken;
                    }
                    None
                }
            };
            let l = copy.as_ref().unwrap_or(&e.value.layout);
            if p.bytes.end < w.text.len() {
                // A part ends in a newline, after which Parley puts an empty line: the next
                // part's first line takes its place, below this part's lines and the room under
                // each of its paragraphs.
                let phantom = l.get(l.len().checked_sub(1)?)?.text_range();
                if !phantom.is_empty() || phantom.start != p.bytes.len() {
                    return None;
                }
                y += l.lines().take(l.len() - 1).map(|line| line.metrics().line_height as f64).sum::<f64>();
                if w.blocks_path {
                    y += p.paragraphs.clone().map(|k| after(w.blocks, k, w.scale)).sum::<f64>();
                }
            }
            copies.push(copy);
        }
        if shaped {
            self.shaped += 1;
        } else if rebroken {
            self.rebroken += 1;
        }
        let mut out = self.paragraphs.reuse();
        out.join_begin(&self.paragraphs.parts.get(&keys[0])?.value.layout);
        let mut chars = 0;
        for (((p, key), &y), copy) in pieces.iter().zip(&keys).zip(&tops).zip(&copies) {
            let l = match copy {
                Some(l) => l,
                None => &self.paragraphs.parts.get(key)?.value.layout,
            };
            let keep = if p.bytes.end == w.text.len() { l.len() } else { l.len() - 1 };
            let dy = (y - y.fract()) as f32;
            let span_of = &p.span_of;
            let mut brush = |b: &Ink| {
                let (i, bold) = (b.0 & !SYNTHETIC_BOLD, b.0 & SYNTHETIC_BOLD);
                Ink(if i == 0 { 0 } else { (span_of[i as usize - 1] as u16).wrapping_add(1) } | bold)
            };
            let pill_of = &p.pill_of;
            let mut box_id = |id: u64| {
                if id & PILL_BOX == 0 {
                    return id;
                }
                let local = ((id & !PILL_BOX) >> 1) as usize;
                PILL_BOX | (pill_of[local] as u64) << 1 | (id & 1)
            };
            if !out.join_part(l, chars, keep, dy, &mut brush, &mut box_id) {
                return None;
            }
            chars += p.chars;
        }
        out.join_end();
        out.align(w.align, AlignmentOptions::default());
        let l = Rc::new(out);
        self.layouts.insert(w.key, layout_cache::Entry::new(l.clone(), g, w.text.len(), w.shape_key));
        self.paragraphs.joined.insert(w.key, keys.into());
        if self.paragraphs.spare.len() >= SPARE {
            self.paragraphs.spare.remove(0);
        }
        self.paragraphs.spare.push(l.clone());
        Some(l)
    }

    /// `w`'s parts, each with its spans and pills, or None when it must shape whole.
    fn pieces(&mut self, w: &Whole<'_>) -> Option<Vec<Piece>> {
        let starts = paragraph_starts(w.text);
        if starts.len() < 2 {
            return None;
        }
        if self.paragraphs.facts.len() > MAX_FACTS {
            self.paragraphs.facts.clear();
        }
        let facts: Vec<Facts> = starts
            .iter()
            .enumerate()
            .map(|(k, &s)| {
                let p = &w.text[s..starts.get(k + 1).copied().unwrap_or(w.text.len())];
                *self.paragraphs.facts.entry(run_hash(p)).or_insert_with(|| facts_of(p))
            })
            .collect();
        let cut = cuts(&facts)?;
        let mut pieces: Vec<Piece> = Vec::new();
        for (k, &s) in starts.iter().enumerate() {
            if cut[k] {
                pieces.push(Piece { bytes: s..s, paragraphs: k..k, chars: 0, spans: Vec::new(), span_of: Vec::new(), pills: Vec::new(), pill_of: Vec::new() });
            }
            let p = pieces.last_mut()?;
            p.bytes.end = starts.get(k + 1).copied().unwrap_or(w.text.len());
            p.paragraphs.end = k + 1;
            p.chars += facts[k].chars as usize;
        }
        if pieces.len() < 2 {
            return None;
        }
        // Each span cut to the parts it covers, in the text's order, so later spans still win.
        let len = w.text.len();
        for (i, s) in w.spans.iter().enumerate() {
            let r = (s.start as usize).min(len)..(s.end as usize).min(len);
            if r.is_empty() {
                continue;
            }
            let first = pieces.partition_point(|p| p.bytes.end <= r.start);
            for p in &mut pieces[first..] {
                if p.bytes.start >= r.end {
                    break;
                }
                let (a, b) = (r.start.max(p.bytes.start), r.end.min(p.bytes.end));
                p.spans.push(Span { start: (a - p.bytes.start) as u32, end: (b - p.bytes.start) as u32, ..s.clone() });
                p.span_of.push(i as u32);
            }
        }
        // A pill lies inside one line.
        for pill in w.pills {
            let at = pieces.partition_point(|p| p.bytes.end <= pill.range.start);
            let p = pieces.get_mut(at)?;
            if pill.range.end > p.bytes.end {
                return None;
            }
            let o = p.bytes.start;
            p.pills.push(PillBox { index: p.pills.len(), range: pill.range.start - o..pill.range.end - o, ..pill.clone() });
            p.pill_of.push(pill.index);
        }
        Some(pieces)
    }

    /// Keeps the parts of the joined layout under `key` while it is used.
    pub(super) fn touch_joined(&mut self, key: u64) {
        let Some(keys) = self.paragraphs.joined.get(&key) else { return };
        let g = self.generation;
        for k in keys.iter() {
            if let Some(e) = self.paragraphs.parts.get_mut(k) {
                e.used = g;
            }
        }
    }

    /// Whether the layout under `key` was joined from paragraphs.
    pub(super) fn is_joined(&self, key: u64) -> bool {
        self.paragraphs.joined.contains_key(&key)
    }

    /// [`Self::end_commit`]'s share: parts unused for two commits go once they outgrow the
    /// stale budget, and joined layouts no longer cached forget their parts.
    pub(super) fn end_commit_paragraphs(&mut self, g: u32) {
        evict(&mut self.paragraphs.parts, g);
        let layouts = &self.layouts;
        self.paragraphs.joined.retain(|k, _| layouts.contains_key(k));
    }
}

/// A part's key: its text, its spans' and pills' shaping fields, and the whole text's facts.
fn part_key(w: &Whole<'_>, p: &Piece, whole: WholeFacts) -> u64 {
    let text = &w.text[p.bytes.clone()];
    let mut h = KeyHasher::default();
    (text_hash(text, &p.spans, w.style, w.scale, None), w.wrap_anywhere, hash::directed(0, w.base), whole).hash(&mut h);
    for pill in &p.pills {
        (pill.index, pill.range.start, pill.range.end, pill.lead, pill.trail).hash(&mut h);
    }
    h.finish()
}

#[cfg(test)]
#[path = "paragraph_join_tests.rs"]
mod tests;
