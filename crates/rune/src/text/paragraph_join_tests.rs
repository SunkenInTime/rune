//! A text joined from its paragraphs lays out as the whole text shaped at once: the same
//! glyphs at the same places, the same lines, carets, hit tests, selection boxes and
//! decorations, and the same instances drawn; and stays so through thousands of edits.
use super::*;
use crate::edit::Block;
use parley::editing::Cursor;
use parley::layout::Affinity;

/// A text system that joins every multi-paragraph text it can, and one that never does.
fn systems() -> (TextSystem, TextSystem) {
    let (mut joined, mut whole) = (TextSystem::default(), TextSystem::default());
    joined.paragraphs.min_bytes = 0;
    whole.paragraphs.min_bytes = usize::MAX;
    (joined, whole)
}

/// Everything about `l` as bits: its size, each line's range, break and metrics, and each glyph
/// run's font, size, place, ink and glyphs, and each inline box's place.
fn geometry(l: &Layout<Ink>) -> Vec<Vec<u64>> {
    let b = |v: f32| v.to_bits() as u64;
    // Fonts by their order of first use: two text systems load the same files as other blobs.
    let mut fonts: Vec<u64> = Vec::new();
    let mut font = |id: u64| fonts.iter().position(|&f| f == id).unwrap_or_else(|| { fonts.push(id); fonts.len() - 1 }) as u64;
    let mut out = vec![vec![b(l.full_width()), b(l.width()), b(l.height()), l.len() as u64, b(l.scale())]];
    for line in l.lines() {
        let m = line.metrics();
        let r = line.text_range();
        out.push(vec![
            r.start as u64, r.end as u64, line.break_reason() as u64,
            b(m.line_height), b(m.baseline), b(m.offset), b(m.indent), b(m.advance), b(m.hanging_advance),
            b(m.inline_min_coord), b(m.inline_max_coord), b(m.block_min_coord), b(m.block_max_coord),
            b(m.content_block_min_coord), b(m.content_block_max_coord),
        ]);
        for item in line.items() {
            match item {
                PositionedLayoutItem::GlyphRun(gr) => {
                    let run = gr.run();
                    let tr = run.text_range();
                    let mut v = vec![
                        font(run.font().data.id()), run.font().index as u64, b(run.font_size()), b(gr.baseline()), b(gr.offset()), b(gr.advance()),
                        gr.style().brush.0 as u64, tr.start as u64, tr.end as u64, run.is_rtl() as u64,
                    ];
                    v.extend(run.normalized_coords().iter().map(|c| c.to_bits() as u16 as u64));
                    v.extend(gr.positioned_glyphs().flat_map(|p| [p.id as u64, b(p.x), b(p.y), b(p.advance)]));
                    out.push(v);
                }
                PositionedLayoutItem::InlineBox(x) => out.push(vec![x.id, b(x.x), b(x.y), b(x.width), b(x.height)]),
            }
        }
    }
    out
}

/// The caret at every character boundary both ways, a hit test on a grid over the layout, and
/// the selection boxes of ranges across it.
fn editing(l: &Layout<Ink>, text: &str) -> Vec<Vec<u64>> {
    let b = |v: f64| v.to_bits();
    let bb = |r: parley::BoundingBox| [b(r.x0), b(r.y0), b(r.x1), b(r.y1)];
    let mut out = Vec::new();
    for (i, _) in text.char_indices().chain([(text.len(), ' ')]) {
        for aff in [Affinity::Downstream, Affinity::Upstream] {
            let c = Cursor::from_byte_index(l, i, aff);
            let mut v = vec![c.index() as u64, (c.affinity() == Affinity::Upstream) as u64];
            v.extend(bb(cursor_geometry(&c, l, 1.0)));
            out.push(v);
        }
    }
    let (w, h) = (l.full_width().max(1.0), l.height().max(1.0));
    for yi in 0..=60 {
        for xi in 0..=24 {
            let (x, y) = (xi as f32 * (w + 40.0) / 24.0 - 20.0, yi as f32 * (h + 40.0) / 60.0 - 20.0);
            let c = cursor_from_point(l, x, y);
            out.push(vec![c.index() as u64, (c.affinity() == Affinity::Upstream) as u64]);
        }
    }
    let n = text.len();
    let mut cuts: Vec<usize> = (0..=16).map(|k| n * k / 16).collect();
    cuts.extend(text.match_indices('\n').map(|(i, _)| i).take(40));
    cuts.retain(|&c| text.is_char_boundary(c));
    for (k, &a) in cuts.iter().enumerate() {
        for &e in &cuts[k..] {
            for pills in [false, true] {
                let mut v = vec![a as u64, e as u64];
                for (r, line) in range_boxes(l, a, e, pills) {
                    v.extend(bb(r));
                    v.push(line as u64);
                }
                out.push(v);
            }
        }
        let mut v = vec![line_edge(l, a, false) as u64, line_edge(l, a, true) as u64];
        v.extend([move_line(l, a, true), move_line(l, a, false)].map(|m| m.map_or(u64::MAX, |m| m as u64)));
        out.push(v);
    }
    out
}

fn decorations(ts: &TextSystem, l: &Layout<Ink>, spans: &[Span]) -> Vec<[u32; 5]> {
    let mut out = Vec::new();
    ts.decorations(l, spans, 2.0, [3.0, 5.0], &mut out);
    out.iter().map(|d: &Deco| [d.rect[0].to_bits(), d.rect[1].to_bits(), d.rect[2].to_bits(), d.rect[3].to_bits(), d.radius.to_bits()]).collect()
}

struct Case {
    name: &'static str,
    text: String,
    spans: Vec<Span>,
    blocks: Vec<Block>,
    pills: Vec<PillChip>,
    style: TextStyle,
    /// Whether it must shape whole (right-to-left text): then nothing joins.
    whole: bool,
}

const WORDS: [&str; 16] = ["the", "quick", "brown", "fox", "jumps", "over", "a", "lazy", "dog", "while", "writing", "prose", "about", "focus", "and", "calm"];

/// A Markdown draft as Fluent Writer writes one: headings, prose, lists, quotes, code, empty
/// paragraphs, emoji, accents and curly quotes, a very long paragraph and a trailing newline.
fn draft(paragraphs: usize) -> String {
    let mut out = String::new();
    for p in 0..paragraphs {
        match p % 9 {
            0 => out.push_str(&format!("## Section {p}\n\n")),
            3 => out.push_str("- a list item that runs long enough to wrap onto a second line of the field\n- another\n"),
            5 => out.push_str("> “Quoted” café — naïve 😀 emoji 👍🏽, ff fi ligatures, 1st -> arrow…\n\n"),
            7 => out.push_str("```\nfn main() {\n\tlet x = 1;\t// a tab\n}\n```\n"),
            _ => {}
        }
        let words = if p == 4 { 900 } else { 60 + p % 23 };
        for w in 0..words {
            out.push_str(WORDS[(p * 7 + w * 13) % WORDS.len()]);
            out.push(if w + 1 == words { '.' } else { ' ' });
        }
        out.push_str("\n\n");
    }
    out
}

/// Bold, italic, mono, sized, coloured, underlined and backgrounded spans over `text`, some
/// overlapping and some across paragraph ends; a sentence dimmed around byte `focus`.
fn spans_of(text: &str, focus: usize) -> Vec<Span> {
    let mut spans = Vec::new();
    let grey = Color { r: 0.5, g: 0.5, b: 0.5, a: 1.0 };
    let mut starts = Vec::new();
    let mut prev = ' ';
    for (i, c) in text.char_indices() {
        if c.is_alphanumeric() && !prev.is_alphanumeric() {
            starts.push(i);
        }
        prev = c;
    }
    for (i, &at) in starts.iter().step_by(7).enumerate() {
        let end = text[at..].find(|c: char| !c.is_alphanumeric()).map_or(text.len(), |e| at + e);
        let s = Span::new(at, end);
        spans.push(match i % 7 {
            0 => s.bold(),
            1 => s.italic(),
            2 => s.mono().bg(grey, 2.0),
            3 => s.size(13.0),
            4 => s.color(Color { r: 0.8, g: 0.2, b: 0.1, a: 1.0 }),
            5 => Span { underline: Some(grey), ..s },
            _ => s.weight(700.0).italic(),
        });
    }
    for (at, _) in text.match_indices("\n\n").step_by(5) {
        let (mut a, mut b) = (at.saturating_sub(12), (at + 14).min(text.len()));
        while !text.is_char_boundary(a) { a -= 1; }
        while !text.is_char_boundary(b) { b += 1; }
        spans.push(Span::new(a, b).line_bg(grey));
    }
    let mut focus = focus.min(text.len());
    while !text.is_char_boundary(focus) { focus -= 1; }
    let a = text[..focus].rfind(". ").map_or(0, |i| i + 2);
    let b = text[focus..].find(". ").map_or(text.len(), |i| focus + i + 1);
    spans.push(Span::new(0, a).color(grey));
    spans.push(Span::new(b, text.len()).color(grey));
    spans
}

/// Each paragraph's block as the app works it out: hanging indents with room under for
/// list items, a quote, code and headings.
fn blocks_of(text: &str) -> Vec<Block> {
    text.split('\n')
        .map(|p| match p.as_bytes().first() {
            Some(b'-') => Block::hang(0.0, 14.5, 3.0),
            Some(b'>') => Block::Quote,
            Some(b'#') => Block::hang(0.0, 0.0, 6.25),
            Some(b'`') => Block::Code,
            Some(b'f' | b'b') => Block::Bullet(1),
            _ => Block::Paragraph,
        })
        .collect()
}

fn cases() -> Vec<Case> {
    let body = TextStyle::new(17.0, 400.0).lh(25.5);
    let text = draft(14);
    let mut out = vec![
        Case { name: "plain draft", text: text.clone(), spans: Vec::new(), blocks: Vec::new(), pills: Vec::new(), style: body, whole: false },
        Case { name: "rich draft", spans: spans_of(&text, text.len() / 3), blocks: blocks_of(&text), text: text.clone(), pills: Vec::new(), style: body, whole: false },
    ];
    // No trailing newline, empty paragraphs at the start, a Greek and a Cyrillic paragraph and
    // ones that start with neutral characters after them, and an IME preedit spliced in.
    let mut odd = String::from("\n\n“Neutral start” after empty paragraphs.\n");
    odd.push_str(&text[..3000]);
    odd.push_str("\nΚαλημέρα κόσμε, ένα ελληνικό κείμενο.\n— a dash after Greek.\n\n123 digits.\nПривет мир.\n(paren after Cyrillic)\nにほんご preedit mid-line, then Latin.\n“quote” after it\nend without newline");
    let mut with_pills = odd.clone();
    with_pills.push_str("\nask @notes and @src/main.rs today, a pill per line as a chat field holds them\n");
    let pill_list: Vec<PillChip> = ["@notes", "@src/main.rs"]
        .iter()
        .map(|label| {
            let i = with_pills.rfind(label).unwrap();
            PillChip { start: i as u32, end: (i + label.len()) as u32, icon: None, bg: None, fg: None, bare: None }
        })
        .collect();
    out.push(Case { name: "scripts and preedit", spans: spans_of(&with_pills, 1500), blocks: blocks_of(&with_pills), text: with_pills, pills: pill_list, style: body, whole: false });
    let mut rtl = text[..2500].to_owned();
    rtl.push_str("\nשלום עולם, a right-to-left paragraph.\nmore text after it\n");
    out.push(Case { name: "right to left", spans: spans_of(&rtl, 10), blocks: blocks_of(&rtl), text: rtl, pills: Vec::new(), style: body, whole: true });
    let mono = TextStyle::new(13.0, 400.0).family(Family::Mono).lh(19.0);
    let code: String = (0..120).map(|i| format!("\tline {i}:\tlet value_{i} = compute(\"{i}\")->more; // a comment that wraps at narrow widths\n")).collect();
    out.push(Case { name: "mono code", spans: Vec::new(), blocks: Vec::new(), pills: Vec::new(), text: code, style: mono, whole: false });
    out
}

fn lay(ts: &mut TextSystem, c: &Case, scale: f32, width: f32, align: Alignment) -> Rc<Layout<Ink>> {
    ts.layout_keyed(None, &c.text, &c.spans, &c.blocks, &c.pills, c.style, scale, Some(width * scale), align, false, true, parley::BaseDirection::Auto)
}

/// The same multi-paragraph rich texts laid out whole and joined from paragraphs match in
/// every glyph, line, caret, hit test, selection box and decoration, at both scales, several
/// widths and alignments.
#[test]
fn a_joined_text_lays_out_as_the_whole_text() {
    for c in cases() {
        for scale in [1.0, 2.0] {
            for (width, align) in [(613.0, Alignment::Start), (377.5, Alignment::Start), (900.0, Alignment::Center), (240.0, Alignment::End), (520.0, Alignment::Justify)] {
                let (mut joined, mut whole) = systems();
                let a = lay(&mut joined, &c, scale, width, align);
                let b = lay(&mut whole, &c, scale, width, align);
                let what = format!("{} at {scale}x, {width} pt, {align:?}", c.name);
                assert_eq!(!joined.paragraphs.joined.is_empty(), !c.whole, "{what}: joined");
                assert!(whole.paragraphs.joined.is_empty());
                let (ga, gb) = (geometry(&a), geometry(&b));
                assert_eq!(ga.len(), gb.len(), "{what}: items");
                for (i, (x, y)) in ga.iter().zip(&gb).enumerate() {
                    if x != y {
                        let at = x.iter().zip(y).position(|(p, q)| p != q).unwrap_or(x.len().min(y.len()));
                        panic!("{what}: item {i} of {}, field {at}: {:?} vs {:?}\n{x:?}\n{y:?}", ga.len(), x.get(at), y.get(at));
                    }
                }
                assert_eq!(editing(&a, &c.text), editing(&b, &c.text), "{what}: editing");
                assert_eq!(decorations(&joined, &a, &c.spans), decorations(&whole, &b, &c.spans), "{what}: decorations");
                assert_eq!(a.calculate_content_widths().max.to_bits(), b.calculate_content_widths().max.to_bits(), "{what}: content width");
            }
        }
    }
}

/// A colour change, a moved highlight and a span added before others reshape at most the
/// paragraphs they touch; a new width re-breaks without shaping.
#[test]
fn only_touched_paragraphs_shape_again() {
    let (mut ts, _) = systems();
    let c = &cases()[1];
    let style = c.style;
    let lay = |ts: &mut TextSystem, spans: &[Span], width: f32| {
        ts.layout_keyed(None, &c.text, spans, &c.blocks, &[], style, 2.0, Some(width), Alignment::Start, false, true, parley::BaseDirection::Auto)
    };
    lay(&mut ts, &c.spans, 1200.0);
    let parts = ts.paragraphs.parts.len();
    assert!(parts > 20, "{parts} parts");
    // The sentence dimming moves within a paragraph: that paragraph shapes again.
    let moved = spans_of(&c.text, c.text.len() / 3 + 40);
    ts.end_commit();
    let before = ts.paragraphs.parts.len();
    lay(&mut ts, &moved, 1200.0);
    assert!(ts.paragraphs.parts.len() - before <= 1, "{} new parts", ts.paragraphs.parts.len() - before);
    // A colour-only change of every span reshapes nothing.
    let recoloured: Vec<Span> = moved.iter().map(|s| Span { color: s.color.map(|_| Color { r: 0.1, g: 0.6, b: 0.2, a: 1.0 }), ..s.clone() }).collect();
    let shaped = ts.shaped;
    let l = lay(&mut ts, &recoloured, 1200.0);
    assert_eq!(ts.shaped, shaped);
    assert_eq!(geometry(&l), geometry(&systems().1.layout_keyed(None, &c.text, &recoloured, &c.blocks, &[], style, 2.0, Some(1200.0), Alignment::Start, false, true, parley::BaseDirection::Auto)));
    // A new width re-breaks every paragraph without shaping one.
    let (shaped, rebroken) = (ts.shaped, ts.rebroken);
    let l = lay(&mut ts, &recoloured, 777.0);
    assert_eq!((ts.shaped, ts.rebroken), (shaped, rebroken + 1));
    assert_eq!(geometry(&l), geometry(&systems().1.layout_keyed(None, &c.text, &recoloured, &c.blocks, &[], style, 2.0, Some(777.0), Alignment::Start, false, true, parley::BaseDirection::Auto)));
}

/// A small deterministic generator, so a failure replays.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// Thousands of random typed characters, deletions and pastes at random places, each laid out
/// by one text system that keeps its paragraph layouts from edit to edit and compared with the
/// whole text shaped afresh.
#[test]
fn random_edits_keep_the_joined_layout_equal_to_a_fresh_one() {
    let (mut ts, _) = systems();
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut text = draft(5);
    let style = TextStyle::new(17.0, 400.0).lh(25.5);
    let typed = ['a', 'e', ' ', '.', '\n', 'T', '-', '#', '“', 'é', '😀', 'Ω', '\t', '1'];
    let pastes = ["\n\n", "pasted words here", "line one\nline two\n\n- item\n", "## Heading\n", "„quoted“ ", "\nΚαλή\n", "x"];
    for step in 0..2500 {
        let mut at = rng.below(text.len() + 1);
        while !text.is_char_boundary(at) {
            at -= 1;
        }
        match rng.below(10) {
            0..=4 => text.insert(at, typed[rng.below(typed.len())]),
            5..=7 => {
                let long = rng.below(8) == 0;
                let mut end = (at + 1 + rng.below(if long { 400 } else { 6 })).min(text.len());
                while !text.is_char_boundary(end) {
                    end += 1;
                }
                text.replace_range(at..end, "");
            }
            _ => text.insert_str(at, pastes[rng.below(pastes.len())]),
        }
        if text.len() < 500 {
            text.push_str(&draft(1));
        }
        let spans = spans_of(&text, rng.below(text.len() + 1));
        let blocks = blocks_of(&text);
        let scale = if step % 3 == 0 { 1.0 } else { 2.0 };
        let width = [613.0, 400.0, 613.0, 613.0][step % 4] * scale;
        let a = ts.layout_keyed(None, &text, &spans, &blocks, &[], style, scale, Some(width), Alignment::Start, false, true, parley::BaseDirection::Auto);
        let b = systems().1.layout_keyed(None, &text, &spans, &blocks, &[], style, scale, Some(width), Alignment::Start, false, true, parley::BaseDirection::Auto);
        assert_eq!(geometry(&a), geometry(&b), "step {step}: {text:?}");
        if step % 50 == 0 {
            assert_eq!(editing(&a, &text), editing(&b, &text), "step {step}");
        }
        ts.end_commit();
    }
    // Edited paragraphs' old layouts go once they outgrow the stale budget.
    let bytes: usize = ts.paragraphs.parts.values().map(|e| e.cost as usize).sum();
    assert!(bytes < 2 * STALE_BYTES, "{} parts kept, {bytes} bytes", ts.paragraphs.parts.len());
}

/// A long rich `text_input` with a caret, a selection and an IME preedit draws the same
/// instances (glyph quads, caret, selection, backgrounds) whether its text joins from
/// paragraphs or shapes whole, before and after typing into it and scrolling it.
#[test]
fn a_long_text_input_draws_the_same_joined_or_whole() {
    use crate::edit::{Ime, TextEdit};
    use crate::{Cx, El, Env, Runtime, ScrollSpec, col};
    struct S {
        edit: TextEdit,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let text = s.edit.text.clone();
        let style = TextStyle::new(17.0, 400.0).lh(25.5);
        let input = crate::el::text_input(&s.edit, style).w_full().spans(spans_of(&text, text.len() / 2)).paragraph_blocks(blocks_of(&text)).on_edit(|s: &mut S| &mut s.edit);
        col().key("scroll").w_full().h_full().scroll(ScrollSpec { stick_bottom: false }).children([input])
    }
    let instances = |rt: &Runtime<S>| -> Vec<Vec<u32>> {
        rt.instances
            .iter()
            .map(|i| [i.rect, i.radii, i.fill, i.border, i.params, i.uv, i.clip].iter().flatten().map(|v| v.to_bits()).collect())
            .collect()
    };
    let run = |join: bool| {
        let mut edit = TextEdit::new(draft(16));
        edit.focused = true;
        edit.set_selection(5000, 5000);
        edit.ime(&Ime::Preedit("にほんご".into(), None));
        let mut rt = Runtime::new(S { edit }, view, (760.0, 900.0), Env::new(true, 2.0));
        rt.text.paragraphs.min_bytes = if join { 0 } else { usize::MAX };
        rt.text.drop_shaped();
        let mut frames = Vec::new();
        rt.frame(0.0);
        frames.push(instances(&rt));
        rt.update(|s, _| {
            s.edit.ime(&Ime::Commit("日本".into()));
            s.edit.insert("typed into the middle of a long draft\n\nand a new paragraph");
            s.edit.set_selection(2000, 2600);
        });
        rt.frame(1.0 / 60.0);
        frames.push(instances(&rt));
        for _ in 0..10 {
            rt.wheel(300.0, 400.0, 120.0);
            rt.frame(1.0 / 60.0);
        }
        frames.push(instances(&rt));
        (frames, rt.text.paragraphs.joined.len())
    };
    let (joined, n) = run(true);
    let (whole, m) = run(false);
    assert!(n > 0 && m == 0, "joined {n}, whole {m}");
    assert_eq!(joined.len(), whole.len());
    for (k, (a, b)) in joined.iter().zip(&whole).enumerate() {
        assert_eq!(a.len(), b.len(), "frame {k}: instance count");
        for (i, (x, y)) in a.iter().zip(b).enumerate() {
            assert_eq!(x, y, "frame {k}: instance {i}");
        }
    }
}
