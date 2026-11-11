//! Shaping a text into a Parley layout: [`TextSystem::layout_keyed`], which every rich layout
//! entry point goes through, with its caches, font fallbacks, `trak` tracking, tabs and pills.

use super::*;

/// A pill's label range and the room before and after it for its chip, in device px. Its
/// boxes keep `index`, the pill's place in the text's pill list, in their id.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct PillBox {
    pub(super) index: usize,
    pub(super) range: std::ops::Range<usize>,
    pub(super) lead: u32,
    pub(super) trail: u32,
}

/// The pills of `text` that keep a chip: ranges that fail the checks keep none.
pub(super) fn pill_boxes(text: &str, pills: &[PillChip], size: f32, scale: f32) -> Vec<PillBox> {
    let pm = PillMetrics::new(size);
    pills
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let r = (p.start as usize).min(text.len())..(p.end as usize).min(text.len());
            let (lead, trail) = match &p.bare {
                Some(b) => (b.size + b.gap, 0.0),
                None => (pm.lead(p.icon.is_some()), pm.trail()),
            };
            PillBox { index: i, range: r, lead: (lead * scale).round() as u32, trail: (trail * scale).round() as u32 }
        })
        .filter(|p| {
            let r = &p.range;
            !r.is_empty() && text.is_char_boundary(r.start) && text.is_char_boundary(r.end) && !text.as_bytes()[r.clone()].contains(&b'\n')
        })
        .collect()
}

/// What shaping found when it checked each run's spacing against its font's `trak` table.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Probe {
    /// A run's font needs other spacing than its style range predicted.
    pub(super) mismatch: bool,
    /// Some run's font needs nonzero spacing.
    pub(super) exact_nonzero: bool,
}

/// How [`TextSystem::shape_text`] treats runs whose font needs other spacing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Exact {
    /// Shapes again with each run's own spacing when one needs it: a whole text.
    Local,
    /// Keeps the first shaping: a paragraph whose text decides with its other paragraphs.
    Probe,
    /// Shapes again with each run's own spacing, `nonzero` when any run of the whole text
    /// needs nonzero spacing.
    Force { nonzero: bool },
}

/// What shaping a paragraph alone would decide differently from shaping the whole text it is
/// part of: whether spacing that only corrects `trak` turns optional ligatures back on, whether
/// inline code keeps inside the strut (any mono span), and whether Windows picks system faces
/// by weight (any non-ASCII text).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct WholeFacts {
    pub(super) corrected: bool,
    pub(super) any_mono: bool,
    pub(super) non_ascii: bool,
}

/// The letter spacing text in `style` at `scale` gets at a size from a font's `trak` table
/// (see `trak.rs`), on top of `style.tracking`. An explicit optical size `o` picks the entry
/// instead, as Core Text tracks `o`-point text scaled to the set size; spans scale the lookup
/// with their size.
fn tracking(style: TextStyle, scale: f32) -> impl Fn(Option<Rc<Trak>>, f32) -> f32 {
    let trak_tag = parley::setting::Tag::new(b"trak");
    let tracked = parley::FontFeature::parse_css_list(style.features).flatten().filter(|f| f.tag == trak_tag).last().is_none_or(|f| f.value != 0);
    let optical = match style.optical_size {
        OpticalSize::Fixed(o) if o.is_finite() && o > 0.0 && style.size > 0.0 => Some(o),
        _ => None,
    };
    move |trak: Option<Rc<Trak>>, size: f32| {
        let px = size * scale;
        let points = px / scale;
        // `o` itself at the style's size (a run's size comes back through device px).
        let points = optical.map_or(points, |o| {
            if (points - style.size).abs() <= style.size * 1e-6 { o } else { o * points / style.size }
        });
        style.tracking + trak.map_or(0.0, |t| t.spacing(px, points, tracked) / scale)
    }
}

fn variations(style: TextStyle, size: f32) -> Vec<parley::FontVariation> {
    let mut settings: Vec<_> = parley::FontVariation::parse_css_list(style.variations).flatten().collect();
    let opsz = parley::setting::Tag::new(b"opsz");
    let value = match style.optical_size {
        OpticalSize::Auto if !settings.iter().any(|v| v.tag == opsz) => Some(size),
        OpticalSize::Fixed(value) => { settings.retain(|v| v.tag != opsz); Some(value) }
        _ => None,
    };
    if let Some(value) = value.filter(|v| v.is_finite() && *v > 0.0) {
        settings.push(parley::FontVariation::new(opsz, value));
    }
    settings
}

/// The byte of each tab in monospace text (mono spans, or all of it in a mono style) with the
/// columns it fills to reach the next 4-column stop. Columns count from the start of the line
/// or of the monospace run, whichever is later.
fn mono_tab_stops(text: &str, spans: &[Span], mono_style: bool) -> Vec<(usize, u32)> {
    let mut regions: Vec<(usize, usize)> = if mono_style {
        vec![(0, text.len())]
    } else {
        let mut r: Vec<_> = spans.iter().filter(|s| s.mono && s.start < s.end).map(|s| (s.start as usize, (s.end as usize).min(text.len()))).collect();
        r.sort_unstable();
        r
    };
    regions.dedup_by(|next, prev| {
        let joined = next.0 <= prev.1;
        if joined {
            prev.1 = prev.1.max(next.1);
        }
        joined
    });
    let mut out = Vec::new();
    for (start, end) in regions {
        let Some(run) = text.get(start..end) else { continue };
        let mut column = 0u32;
        for (i, ch) in run.char_indices() {
            match ch {
                '\n' => column = 0,
                '\t' => {
                    let fill = 4 - column % 4;
                    out.push((start + i, fill));
                    column += fill;
                }
                _ => column += 1,
            }
        }
    }
    out
}

impl TextSystem {
    /// `shape` is [`shape_key`] of the same text when the caller already has it.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn layout_keyed(
        &mut self,
        shape: Option<u64>,
        text: &str,
        spans: &[Span],
        blocks: &[crate::edit::Block],
        pills: &[PillChip],
        style: TextStyle,
        scale: f32,
        max_width: Option<f32>,
        align: Alignment,
        wrap_anywhere: bool,
        cache: bool,
        base: parley::BaseDirection,
    ) -> Rc<Layout<Ink>> {
        self.follow_language();
        let shared = (!cache).then(|| std::mem::replace(&mut self.layout_cx, LayoutContext::new()));
        let has_indented_blocks = blocks.iter().any(|&b| b != crate::edit::Block::Paragraph);
        // Shaping depends on the text, spans, style, scale, overflow-wrap and pills; line
        // breaking adds the width, alignment and block indents. A layout cached at one width
        // is re-broken at another rather than shaped again.
        let shape_key = shape.unwrap_or_else(|| hash::directed(shape_key(text, spans, style, scale, wrap_anywhere, pills), base));
        let key = hash::break_key(shape_key, max_width, align, has_indented_blocks.then_some(blocks));
        let g = self.generation;
        if cache && let Some(Entry { value: l, used, .. }) = self.layouts.get_mut(&key) {
            *used = g;
            let l = l.clone();
            if self.is_joined(key) {
                // A joined layout keeps its paragraphs, which a new width re-breaks.
                self.touch_joined(key);
            } else {
                // A width change re-breaks the newest layout, not one measured since at another.
                self.shapes.entry(shape_key).or_insert(key);
            }
            return l;
        }
        let pills = pill_boxes(text, pills, style.size, scale);
        let blocks_path = has_indented_blocks || !pills.is_empty();
        let break_lines = |layout: &mut Layout<Ink>| {
            break_paragraphs(layout, blocks_path, blocks, style.size, scale, max_width, 0.0);
        };
        if cache
            && let Some(prev) = self.shapes.get(&shape_key).copied()
            && let Some(Entry { value: shaped, used, .. }) = self.layouts.get(&prev)
        {
            self.rebroken += 1;
            let _break_time = crate::frame::profile::Scope::new(crate::frame::profile::Phase::Text);
            // One this commit hasn't used and nothing else holds moves to the new width and
            // re-breaks in place; otherwise the shaped runs are copied.
            let (moves, mut l) = (*used != g && Rc::strong_count(shaped) == 1, shaped.clone());
            if moves { self.layouts.remove(&prev); self.glyph_cache.forget(&l); }
            let layout = Rc::make_mut(&mut l);
            break_lines(layout);
            layout.align(align, AlignmentOptions::default());
            self.layouts.insert(key, Entry::new(l.clone(), g, text.len(), shape_key));
            self.shapes.insert(shape_key, key);
            return l;
        }
        if cache && let Some(l) = self.layout_paragraphs(&paragraph_join::Whole {
            text, spans, blocks, pills: &pills, style, scale, max_width, align, wrap_anywhere, base, blocks_path, shape_key, key,
        }) {
            return l;
        }
        self.shaped += 1;
        let _shape_time = crate::frame::profile::Scope::new(crate::frame::profile::Phase::Text);
        let (mut layout, _) = self.shape_text(text, spans, &pills, style, scale, wrap_anywhere, base, &break_lines, None, Exact::Local);
        layout.align(align, AlignmentOptions::default());
        let l = Rc::new(layout);
        if cache {
            self.layouts.insert(key, Entry::new(l.clone(), g, text.len(), shape_key));
            self.shapes.insert(shape_key, key);
        }
        if let Some(shared) = shared {
            self.layout_cx = shared;
        }
        l
    }

    /// Shapes `text` into a layout broken into lines by `break_lines` but not aligned, with
    /// what its `trak` check found ([`Probe`]). A paragraph of a longer text passes `whole`,
    /// what the whole text decides that paragraphs alone would decide otherwise; `exact_mode`
    /// says what to do about runs whose font needs other spacing.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn shape_text(
        &mut self,
        text: &str,
        spans: &[Span],
        pills: &[PillBox],
        style: TextStyle,
        scale: f32,
        wrap_anywhere: bool,
        base: parley::BaseDirection,
        break_lines: &dyn Fn(&mut Layout<Ink>),
        whole: Option<WholeFacts>,
        exact_mode: Exact,
    ) -> (Layout<Ink>, Probe) {
        let mono_style = matches!(style.family, Family::Mono | Family::SystemMono);
        let tabs = if text.contains('\t') { mono_tab_stops(text, spans, mono_style) } else { Vec::new() };
        let widen = if tabs.is_empty() { Vec::new() } else { self.mono_tab_widening(&tabs, spans, style, scale) };
        // Parley has no tab stops and fonts draw a tab as a missing glyph (or nothing): shape it
        // as a space, one byte like the tab, so every byte range still holds. Monospace tabs
        // then widen to the next 4-column stop through word spacing.
        let spaced;
        let text = if text.contains('\t') {
            spaced = zeroize::Zeroizing::new(text.replace('\t', " "));
            spaced.as_str()
        } else {
            text
        };
        let routes = fallback::routes(text);
        if !text.is_ascii() {
            let (mut scripts, mut symbols) = (0u64, false);
            for ch in text.chars() {
                for (i, fonts) in fallback::SCRIPT_FONTS.iter().enumerate() {
                    if (self.scripts | scripts) & (1 << i) == 0 && fonts.covers(ch) {
                        scripts |= 1 << i;
                    }
                }
                symbols |= self.math.is_none() && fallback::needs_symbols(ch);
            }
            for (i, fonts) in fallback::SCRIPT_FONTS.iter().enumerate() {
                if scripts & (1 << i) != 0 {
                    self.scripts |= 1 << i;
                    register_script_fallbacks(&mut self.font_cx.collection, &mut self.font_files, fonts);
                }
            }
            if symbols {
                self.math = Some(load_families(&mut self.font_cx.collection, fallback::MATH_FONT_FILES, fallback::MATH_FAMILIES));
            }
        }
        // `trak` tracking at the point size (see `trak.rs`), on top of `style.tracking`. An
        // explicit optical size `o` picks the entry instead, as Core Text tracks `o`-point text
        // scaled to the set size; spans scale the lookup with their size. Each style range gets
        // its predicted first font's spacing; runs in another font are checked after shaping.
        let trak_tag = parley::setting::Tag::new(b"trak");
        let mut features: Vec<parley::FontFeature> = parley::FontFeature::parse_css_list(style.features).flatten().collect();
        let listed = features.len();
        features.retain(|f| f.tag != trak_tag);
        let stripped = features.len() != listed;
        let spacing = tracking(style, scale);
        let (base_spacing, span_spacing) = self.spacings(spans, style, &spacing);
        // Each style range's predicted first font at its weight, drawn at its named instance.
        let base_axes = self.primary_axes(style.family, style.weight, style.italic);
        // The strut inline code keeps inside (`inline_fit`).
        let any_mono = whole.map_or_else(|| spans.iter().any(|s| s.mono), |w| w.any_mono);
        let strut = if mono_style || !any_mono { None } else { self.primary_extents(style.family, style.weight, style.italic) };
        let span_axes: Vec<Rc<[parley::FontVariation]>> = spans
            .iter()
            .map(|s| {
                let family = if s.mono { Family::Mono } else { style.family };
                self.primary_axes(family, s.weight.unwrap_or(style.weight), s.italic || style.italic)
            })
            .collect();
        let with_axes = |mut settings: Vec<parley::FontVariation>, axes: &[parley::FontVariation]| {
            let extra: Vec<_> = axes.iter().filter(|a| !settings.iter().any(|v| v.tag == a.tag)).copied().collect();
            settings.extend(extra);
            settings
        };
        if style.family == Family::SystemMono || self.slot_system_mono {
            self.system_mono();
        }
        if let Family::Named(list) = style.family {
            self.load_installed(list);
        }
        let math = self.math.as_deref().unwrap_or(&[]);
        let emoji = self.emoji.as_slice();
        let system = self.system_mono.as_deref().unwrap_or_default();
        let with_symbols = |primary| {
            family_list(primary, system, self.sans.as_str())
                .into_iter()
                .chain(self.symbol_families.iter().chain(math).map(|name| FontFamilyName::named(name)))
                .collect::<Vec<_>>()
        };
        let sans = match style.family {
            Family::Sans => with_symbols(self.sans.as_str()),
            Family::Mono => with_symbols(self.mono.as_str()),
            Family::Named(list) => {
                let mut families = with_symbols(self.sans.as_str());
                families.splice(0..0, named_list(list));
                families
            }
            Family::SystemMono => {
                let mut families = with_symbols(self.mono.as_str());
                families.splice(0..0, system.iter().map(|name| FontFamilyName::named(name)));
                families
            }
        };
        let mono = with_symbols(self.mono.as_str());
        let language = self.language;
        // Routed clusters try their fallback first, then the run's own stack (mono spans included).
        // Each of the four {math, emoji} × {sans, mono} stacks is built at most once.
        let mut stacks: [Option<Vec<FontFamilyName<'_>>>; 4] = Default::default();
        let mut routed = Vec::new();
        for (range, route) in routes {
            let fonts = match route { Route::Math => math, Route::Emoji => emoji };
            if fonts.is_empty() {
                continue;
            }
            let mono_span = spans.iter().any(|s| s.mono && (s.start as usize) < range.end && range.start < s.end as usize);
            let stack = 2 * (route == Route::Emoji) as usize + mono_span as usize;
            stacks[stack].get_or_insert_with(|| {
                let base = if mono_span { &mono } else { &sans };
                fonts.iter().map(|name| FontFamilyName::named(name)).chain(base.iter().cloned()).collect()
            });
            routed.push((range, stack));
        }
        let span_range = |s: &Span| (s.start as usize).min(text.len())..(s.end as usize).min(text.len());
        // `exact` overrides the predicted spacing with each run's own, by byte range.
        // Code never takes ligatures (`->` stays two characters); later features override earlier.
        let off = [b"liga", b"clig", b"calt", b"dlig"].map(|tag| parley::FontFeature::new(parley::setting::Tag::new(tag), 0));
        let unligated: Vec<parley::FontFeature> = features.iter().chain(&off).copied().collect();
        type Spacings = [(std::ops::Range<usize>, f32)];
        let corrected0 = whole.map_or_else(|| base_spacing != 0.0 || span_spacing.iter().flatten().any(|&v| v != 0.0), |w| w.corrected);
        let build = |lcx: &mut LayoutContext<Ink>, fcx: &mut FontContext, exact: &Spacings, exact_nonzero: bool, widen: &Spacings, weights: &Spacings, fits: &Spacings, bold: &[(std::ops::Range<usize>, u16)]| {
            let mut b = lcx.ranged_builder(fcx, text, scale, true);
            b.set_base_direction(base);
            // Lines break where Chromium breaks them: after every run of spaces, even before a
            // closing bracket or comma (UAX #14 LB13 forbids that), and by Blink's ASCII pair
            // table elsewhere, so "number }" wraps before "}" rather than before "number".
            b.set_line_break_override(Some(parley::CHROMIUM_LINE_BREAK_OVERRIDE));
            b.push_default(FontFamily::from(sans.as_slice()));
            b.push_default(StyleProperty::Locale(language));
            b.push_default(StyleProperty::FontSize(style.size));
            b.push_default(StyleProperty::FontWeight(FontWeight::new(style.weight)));
            if style.italic {
                b.push_default(StyleProperty::FontStyle(FontStyle::Italic));
            }
            // Parley turns optional ligatures off under letter spacing; Core Text's tracking
            // keeps them, so spacing that only corrects `trak` turns them back on.
            let corrected = corrected0 || exact_nonzero;
            if mono_style {
                b.push_default(StyleProperty::FontFeatures(unligated.as_slice().into()));
            } else if style.tracking == 0.0 && corrected {
                let ligatures = [b"liga", b"clig"].map(|tag| parley::FontFeature::new(parley::setting::Tag::new(tag), 1));
                let list = ligatures.iter().chain(&features).copied().collect();
                b.push_default(StyleProperty::FontFeatures(parley::FontFeatures::List(std::borrow::Cow::Owned(list))));
            } else if stripped {
                b.push_default(StyleProperty::FontFeatures(features.as_slice().into()));
            } else if !style.features.is_empty() {
                b.push_default(StyleProperty::FontFeatures(style.features.into()));
            }
            let settings = with_axes(variations(style, style.size), &base_axes);
            if !settings.is_empty() {
                b.push_default(parley::FontVariations::List(std::borrow::Cow::Owned(settings)));
            }
            b.push_default(LineHeight::Absolute(style.line_px()));
            if wrap_anywhere {
                b.push_default(StyleProperty::OverflowWrap(parley::OverflowWrap::Anywhere));
            }
            if base_spacing != 0.0 {
                b.push_default(StyleProperty::LetterSpacing(base_spacing));
            }
            for (i, s) in spans.iter().enumerate() {
                let r = span_range(s);
                if r.is_empty() {
                    continue;
                }
                b.push(StyleProperty::Brush(Ink(i as u16 + 1)), r.clone());
                if s.mono {
                    b.push(FontFamily::from(mono.as_slice()), r.clone());
                    b.push(StyleProperty::FontFeatures(unligated.as_slice().into()), r.clone());
                }
                if let Some(w) = s.weight {
                    b.push(StyleProperty::FontWeight(FontWeight::new(w)), r.clone());
                }
                if s.italic {
                    b.push(StyleProperty::FontStyle(FontStyle::Italic), r.clone());
                }
                if let Some(sz) = s.size {
                    b.push(StyleProperty::FontSize(sz), r.clone());
                    // CSS's unitless line-height: a span of its own size (inline code, a smaller run) keeps the
                    // paragraph's leading ratio, so its box fits inside the line instead of growing it
                    b.push(LineHeight::FontSizeRelative(style.line_px() / style.size), r.clone());
                }
                let resized = s.size.is_some() && style.optical_size == OpticalSize::Auto;
                if resized || span_axes[i] != base_axes {
                    let settings = with_axes(variations(style, s.size.unwrap_or(style.size)), &span_axes[i]);
                    b.push(parley::FontVariations::List(std::borrow::Cow::Owned(settings)), r.clone());
                }
                if let Some(v) = span_spacing[i] {
                    b.push(StyleProperty::LetterSpacing(v), r.clone());
                }
            }
            for (range, w) in weights {
                b.push(StyleProperty::FontWeight(FontWeight::new(*w)), range.clone());
            }
            for (range, lh) in fits {
                b.push(LineHeight::Absolute(*lh), range.clone());
            }
            for (range, ink) in bold {
                b.push(StyleProperty::Brush(Ink(*ink)), range.clone());
            }
            for (range, stack) in &routed {
                b.push(FontFamily::from(stacks[*stack].as_deref().unwrap_or_default()), range.clone());
            }
            for (range, v) in exact {
                b.push(StyleProperty::LetterSpacing(*v), range.clone());
            }
            for (range, v) in widen {
                b.push(StyleProperty::WordSpacing(*v), range.clone());
            }
            // Blink widens each glyph in synthetic bold by a pixel (`SyntheticBoldOffset`).
            for (range, _) in bold {
                let at = range.start;
                let v = exact.iter().find(|(r, _)| r.contains(&at)).map(|e| e.1)
                    .or_else(|| spans.iter().zip(&span_spacing).rev().find_map(|(s, v)| v.filter(|_| span_range(s).contains(&at))))
                    .unwrap_or(base_spacing);
                b.push(StyleProperty::LetterSpacing(v + synthetic_bold::ADVANCE), range.clone());
            }
            // Boxes before and after a pill's label make room for its chip. The label may break
            // like any text; `break_lines_with_blocks` moves a split pill whole to the next line
            // and lets it wrap only when it is wider than a line, as a browser wraps a span.
            for &PillBox { index: i, range: ref r, lead, trail } in pills {
                for (side, index, width) in [(0, r.start, lead), (1, r.end, trail)] {
                    b.push_inline_box(parley::InlineBox {
                        id: PILL_BOX | (i as u64) << 1 | side,
                        kind: parley::InlineBoxKind::InFlow,
                        index,
                        width: width as f32,
                        height: 0.0,
                        baseline: None,
                        vertical_align: parley::VerticalAlign::BASELINE,
                    });
                }
            }
            let mut layout: Layout<Ink> = b.build(text);
            break_lines(&mut layout);
            layout
        };
        let mut layout = build(&mut self.layout_cx, &mut self.font_cx, &[], false, &widen, &[], &[], &[]);
        // On Windows, system faces as DirectWrite picks them for each run's weight, as Chromium.
        let system = cfg!(target_os = "windows") && whole.map_or_else(|| !text.is_ascii(), |w| w.non_ascii);
        let wanted = |at: usize| spans.iter().rev().find_map(|s| s.weight.filter(|_| span_range(s).contains(&at))).unwrap_or(style.weight);
        let weights = if system {
            system_weight::corrections(&mut self.system_faces, &mut self.font_cx, &self.font_files, &layout, wanted)
        } else {
            Vec::new()
        };
        if !weights.is_empty() {
            layout = build(&mut self.layout_cx, &mut self.font_cx, &[], false, &widen, &weights, &[], &[]);
        }
        // Inline code keeps inside the line box its text sets, as a browser's does.
        let fits = strut.map_or_else(Vec::new, |strut| inline_fit::mono_fits(&layout, text, spans, any_mono, style, scale, strut));
        // Bold asked of a system face with no bold draws synthetic bold, as Chromium's does.
        let bolds = cfg!(target_os = "windows") && (style.weight >= 600.0 || spans.iter().any(|s| s.weight.is_some_and(|w| w >= 600.0)));
        let bold = if bolds {
            synthetic_bold::inks(&system_weight::synthetic_bold(&mut self.system_faces, &mut self.font_cx, &self.font_files, &layout, wanted), spans)
        } else {
            Vec::new()
        };
        if !fits.is_empty() || !bold.is_empty() {
            layout = build(&mut self.layout_cx, &mut self.font_cx, &[], false, &widen, &weights, &fits, &bold);
        }
        // The spacing each run got, against the spacing its own font needs.
        let given = |at: usize| {
            spans.iter().zip(&span_spacing).rev().find_map(|(s, v)| v.filter(|_| span_range(s).contains(&at))).unwrap_or(base_spacing)
        };
        let mut exact: Vec<(std::ops::Range<usize>, f32)> = Vec::new();
        let mut mismatch = false;
        for line in layout.lines() {
            for run in line.runs() {
                let font = run.font();
                let trak = font_trak(&mut self.traks, &font.data, font.index);
                let needed = spacing(trak, run.font_size() / scale);
                let range = run.text_range();
                if range.is_empty() {
                    continue;
                }
                mismatch |= (needed - given(range.start)).abs() > 1e-4;
                match exact.last_mut() {
                    Some((last, v)) if last.end == range.start && *v == needed => last.end = range.end,
                    _ => exact.push((range, needed)),
                }
            }
        }
        let probe = Probe { mismatch, exact_nonzero: exact.iter().any(|e| e.1 != 0.0) };
        let rebuild = match exact_mode {
            Exact::Local => mismatch.then_some(probe.exact_nonzero),
            Exact::Probe => None,
            Exact::Force { nonzero } => Some(nonzero),
        };
        if let Some(nonzero) = rebuild {
            layout = build(&mut self.layout_cx, &mut self.font_cx, &exact, nonzero, &widen, &weights, &fits, &bold);
        }
        (layout, probe)
    }

    /// The letter spacing of `style` (its tracking and its first font's `trak`), and of each
    /// span that may shape in another font where it differs.
    fn spacings(&mut self, spans: &[Span], style: TextStyle, spacing: &dyn Fn(Option<Rc<Trak>>, f32) -> f32) -> (f32, Vec<Option<f32>>) {
        let base_spacing = spacing(self.primary_trak(style.family, style.weight, style.italic), style.size);
        let span_spacing = spans
            .iter()
            .map(|s| {
                if !(s.mono || s.italic || s.weight.is_some() || s.size.is_some()) {
                    return None;
                }
                let family = if s.mono { Family::Mono } else { style.family };
                let trak = self.primary_trak(family, s.weight.unwrap_or(style.weight), s.italic || style.italic);
                Some(spacing(trak, s.size.unwrap_or(style.size))).filter(|&v| v != base_spacing)
            })
            .collect();
        (base_spacing, span_spacing)
    }

    /// What [`Self::shape_text`] decides from the whole of `text` (see [`WholeFacts`]).
    pub(super) fn whole_facts(&mut self, text: &str, spans: &[Span], style: TextStyle, scale: f32) -> WholeFacts {
        let (base, span) = self.spacings(spans, style, &tracking(style, scale));
        WholeFacts {
            corrected: base != 0.0 || span.iter().flatten().any(|&v| v != 0.0),
            any_mono: spans.iter().any(|s| s.mono),
            non_ascii: !text.is_ascii(),
        }
    }

    fn mono_tab_widening(
        &mut self,
        tabs: &[(usize, u32)],
        spans: &[Span],
        style: TextStyle,
        scale: f32,
    ) -> Vec<(std::ops::Range<usize>, f32)> {
        let mono_style = matches!(style.family, Family::Mono | Family::SystemMono);
        let default_family = if mono_style { style.family } else { Family::Mono };
        let mut last_probe: Option<((u32, u32, bool), f32)> = None;
        let mut widen: Vec<(std::ops::Range<usize>, f32)> = Vec::new();
        for &(at, columns) in tabs {
            if columns <= 1 {
                continue;
            }
            let (mut sz, mut wt, mut it) = (style.size, style.weight, style.italic);
            for s in spans {
                if (s.start as usize) <= at && at < s.end as usize {
                    if let Some(v) = s.size { sz = v; }
                    if let Some(v) = s.weight { wt = v; }
                    if s.italic { it = true; }
                }
            }
            let probe_key = (sz.to_bits(), wt.to_bits(), it);
            let col_pt = match last_probe {
                Some((k, v)) if k == probe_key => v,
                _ => {
                    let probe = TextStyle { family: default_family, size: sz, weight: wt, italic: it, ..style };
                    let shaped = self.shaped;
                    let l = self.layout_rich(" ", &[], probe, scale, None);
                    self.shaped = shaped;
                    let adv = parley::Cluster::from_byte_index(&l, 0).map_or(0.0, |c| c.advance() / scale);
                    last_probe = Some((probe_key, adv));
                    adv
                }
            };
            if col_pt <= 0.0 {
                continue;
            }
            let add = (columns - 1) as f32 * col_pt;
            match widen.last_mut() {
                Some((r, last_add)) if r.end == at && (*last_add - add).abs() < 1e-4 => r.end = at + 1,
                _ => widen.push((at..at + 1, add)),
            }
        }
        widen
    }
}
