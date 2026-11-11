//! Line-height backgrounds (`Span::line_bg`, `TextHighlight::line_bg`): square fills from each
//! line's top to its bottom over the range's glyphs, meeting their neighbours without a seam.

use crate::render::{Instance, KIND_RECT};
use crate::{Color, Cx, El, Env, Runtime, Span, TextEdit, TextHighlight, TextStyle, col, rich, text, text_input};

const RED: Color = Color { r: 1.0, g: 0.0, b: 0.0, a: 0.5 };
const BLUE: Color = Color { r: 0.0, g: 0.0, b: 1.0, a: 0.5 };

fn fills<S: 'static>(rt: &Runtime<S>, c: Color) -> Vec<[f32; 4]> {
    let fill = c.premul(1.0);
    rt.instances.iter().filter(|i: &&Instance| i.params[1] == KIND_RECT && i.fill == fill).map(|i| i.rect).collect()
}

fn square<S: 'static>(rt: &Runtime<S>, c: Color) -> bool {
    let fill = c.premul(1.0);
    rt.instances.iter().filter(|i| i.params[1] == KIND_RECT && i.fill == fill).all(|i| i.radii == [0.0; 4])
}

/// The line boxes of the text keyed `key`, in window px: (top, bottom) per line.
fn lines<S: 'static>(rt: &Runtime<S>, key: &str) -> Vec<(f32, f32)> {
    let e = rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).expect("keyed text");
    let n = rt.nodes.iter().find(|n| n.key.as_deref() == Some(key)).unwrap();
    let layout = n.text_layout.as_deref().expect("laid out");
    let s = rt.env.scale;
    layout.lines().map(|l| {
        let m = l.metrics();
        (((e.rect[1] * s + m.block_min_coord).round()), (e.rect[1] * s + m.block_max_coord).round())
    }).collect()
}

#[test]
fn a_span_line_bg_fills_each_line_whole_square_and_unpadded() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        // "one two three four" wraps; red covers "one two thr", blue "ee four" right after it.
        col().padding(10.0).child(
            rich("one two three four", vec![Span::new(0, 11).line_bg(RED), Span::new(11, 18).line_bg(BLUE)])
                .key("t").w(60.0).font(TextStyle::new(14.0, 400.0).lh(22.0)),
        )
    }
    let mut rt = Runtime::new((), view, (200.0, 120.0), Env::new(false, 2.0));
    rt.frame(0.0);
    let rows = lines(&rt, "t");
    assert!(rows.len() >= 2, "the text wraps: {rows:?}");
    let (red, blue) = (fills(&rt, RED), fills(&rt, BLUE));
    assert!(square(&rt, RED) && square(&rt, BLUE), "no rounding");
    for r in red.iter().chain(&blue) {
        assert!(rows.iter().any(|&(top, bot)| r[1] == top && r[1] + r[3] == bot), "{r:?} spans a whole line box of {rows:?}");
    }
    // Wrapped lines meet: each line's bottom is the next one's top.
    for w in rows.windows(2) {
        assert_eq!(w[0].1, w[1].0, "line boxes touch");
    }
    // Where red ends and blue starts on one line, they share the edge.
    let joint = red.iter().find_map(|r| blue.iter().find(|b| b[1] == r[1]).map(|b| (r, b)));
    let (r, b) = joint.expect("red and blue share a line");
    assert_eq!(r[0] + r[2], b[0], "touching ranges meet without a gap or an overlap");
}

#[test]
fn a_span_line_bg_is_unpadded_where_bg_pads_and_rounds() {
    fn view(line: &bool, _: &mut Cx<bool>) -> El {
        let span = if *line { Span::new(2, 5).line_bg(RED) } else { Span::new(2, 5).bg(RED, 3.0) };
        col().padding(10.0).child(rich("a bcd e", vec![span]).key("t").font(TextStyle::new(14.0, 400.0)))
    }
    let mut rt = Runtime::new(false, view, (200.0, 60.0), Env::new(false, 1.0));
    rt.frame(0.0);
    let chip = fills(&rt, RED)[0];
    rt.state = true;
    rt.invalidate();
    rt.frame(0.01);
    let line = fills(&rt, RED)[0];
    assert!((chip[2] - line[2] - 6.0).abs() <= 1.0, "the chip pads 3 pt a side, the fill doesn't: {chip:?} {line:?}");
    let (top, bot) = lines(&rt, "t")[0];
    assert_eq!((line[1], line[1] + line[3]), (top, bot));
    assert!(square(&rt, RED));
}

#[test]
fn a_text_highlight_line_bg_matches_the_span_fill_and_skips_the_newline() {
    fn view(span: &bool, _: &mut Cx<bool>) -> El {
        let t = "first line\nsecond";
        let el = if *span { rich(t, vec![Span::new(6, 13).line_bg(RED)]) } else { text(t).text_highlight(TextHighlight::new(6..13, RED).line_bg()) };
        col().padding(10.0).child(el.key("t").font(TextStyle::new(14.0, 400.0)))
    }
    let mut rt = Runtime::new(false, view, (300.0, 80.0), Env::new(false, 2.0));
    rt.frame(0.0);
    let highlight = fills(&rt, RED);
    assert!(square(&rt, RED));
    rt.state = true;
    rt.invalidate();
    rt.frame(0.01);
    let span = fills(&rt, RED);
    assert_eq!(highlight.len(), 2, "one fill a line: {highlight:?}");
    assert_eq!(highlight, span, "a highlight's fill and a span's are the same rects");
    let rows = lines(&rt, "t");
    assert_eq!((highlight[0][1] + highlight[0][3], highlight[1][1]), (rows[0].1, rows[1].0), "the two lines' fills touch");
}

#[test]
fn line_fills_paint_under_rounded_chips_and_rings() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col().padding(10.0).child(
            rich("say code now", vec![Span::new(4, 8).bg(BLUE, 2.0)])
                .text_highlight(TextHighlight::new(0..12, RED).line_bg())
                .font(TextStyle::new(14.0, 400.0)),
        )
    }
    let mut rt = Runtime::new((), view, (300.0, 60.0), Env::new(false, 1.0));
    rt.frame(0.0);
    let at = |c: Color| rt.instances.iter().position(|i| i.params[1] == KIND_RECT && i.fill == c.premul(1.0)).unwrap();
    assert!(at(RED) < at(BLUE), "the line fill is under the inline-code chip");
}

#[test]
fn a_fields_line_bg_span_follows_its_text_through_typing() {
    struct S { edit: TextEdit }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let at = s.edit.text.find("note").unwrap_or(0);
        text_input(&s.edit, TextStyle::new(14.0, 400.0)).key("f").w(200.0).on_edit(|s: &mut S| &mut s.edit)
            .spans(vec![Span::new(at, at + 4).line_bg(RED)])
    }
    let mut rt = Runtime::new(S { edit: TextEdit::new("a note") }, view, (240.0, 60.0), Env::new(false, 1.0));
    rt.frame(0.0);
    let before = fills(&rt, RED);
    assert_eq!(before.len(), 1);
    assert!(square(&rt, RED));
    rt.update(|s, _| s.edit.replace_text("one more note"));
    rt.frame(0.01);
    let after = fills(&rt, RED);
    assert!(after[0][0] > before[0][0] && (after[0][2] - before[0][2]).abs() < 0.5, "the fill moved with its word: {before:?} {after:?}");
    assert_eq!((after[0][1], after[0][3]), (before[0][1], before[0][3]));
}

#[test]
fn see_through_line_fills_show_no_seam_between_lines_or_ranges() {
    use crate::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};
    const PAGE: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 1.0 };
    const SEE: Color = Color { r: 1.0, g: 0.8, b: 0.0, a: 0.4 };
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let t = "Comments anchor to ranges that wrap across lines";
        col().size(200.0, 120.0).fill(PAGE).padding(10.0).child(
            rich(t, vec![Span::new(0, 20).line_bg(SEE), Span::new(20, t.len()).line_bg(SEE)])
                .key("t").w(150.0).font(TextStyle::new(14.0, 400.0).lh(20.0)).color(Color::BLACK),
        )
    }
    let mut rt = Runtime::new((), view, (200.0, 120.0), Env::new(false, 1.5));
    rt.frame(0.0);
    let gpu = Gpu::headless();
    let size = (300u32, 180u32);
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let target = Offscreen::new(&gpu.device, size.0, size.1);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, size);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    renderer.draw(&mut encoder, &target.view, [0.0; 4]);
    target.copy(&mut encoder);
    gpu.queue.submit([encoder.finish()]);
    let out = target.read(&gpu.device);
    if let Ok(path) = std::env::var("RUNE_LINE_BG_PNG") {
        crate::render::write_png(std::path::Path::new(&path), size.0, size.1, &out).unwrap();
    }
    let px = |x: f32, y: f32| -> [u8; 4] { out[(y as usize * size.0 as usize + x as usize) * 4..][..4].try_into().unwrap() };
    let rows = lines(&rt, "t");
    assert!(rows.len() >= 3, "{rows:?}");
    // Left of every line's first glyph is in the fill (x = the text's left edge + 1 px): the
    // pixel on each line boundary, and the line's middle, are the same tint.
    let x = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("t")).unwrap().rect[0] * 1.5 + 1.0;
    let mid = px(x, (rows[0].0 + rows[0].1) * 0.5 - 0.0);
    for w in rows.windows(2) {
        let (above, below) = (px(x, w[0].1 - 1.0), px(x, w[1].0));
        assert_eq!(above, below, "no darker or lighter row where two lines' fills meet");
    }
    assert!(mid[2] < 200, "tinted: {mid:?}");
}

#[test]
fn a_fields_line_bg_span_shifts_across_an_ime_composition_before_it() {
    struct S { edit: TextEdit }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        // Offsets into `edit.text` ("a note"): Rune maps them past the composition.
        text_input(&s.edit, TextStyle::new(14.0, 400.0)).key("f").w(240.0).on_edit(|s: &mut S| &mut s.edit)
            .spans(vec![Span::new(2, 6).line_bg(RED)])
    }
    let mut edit = TextEdit::new("a note");
    edit.focused = true;
    edit.set_caret(0);
    let mut rt = Runtime::new(S { edit }, view, (280.0, 60.0), Env::new(false, 1.0));
    rt.frame(0.0);
    let before = fills(&rt, RED);
    assert_eq!(before.len(), 1);
    rt.update(|s, _| { s.edit.ime(&crate::Ime::Preedit("にほん".into(), None)); });
    rt.frame(0.01);
    let during = fills(&rt, RED);
    assert_eq!(during.len(), 1, "one fill, not split by the composition: {during:?}");
    assert!(during[0][0] > before[0][0] + 10.0 && (during[0][2] - before[0][2]).abs() < 0.5, "moved past the composition: {before:?} {during:?}");
    assert_eq!((during[0][1], during[0][3]), (before[0][1], before[0][3]), "still the line's whole height");
    assert!(square(&rt, RED));
}
