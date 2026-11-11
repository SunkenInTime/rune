//! Parley layout (wrapping, rich spans, registered families) over embedded fonts, swash rasterising
//! glyphs and SVG icons into one greyscale atlas.
//!
//! Invariants: layouts are in device pixels; every layout is cached by a 64-bit hash of
//! (text, spans, style, scale, width), kept for two commits unused and then while a budget of
//! stale layouts lasts (`STALE_BYTES`), least recently used out first; a width not cached re-breaks
//! a cached layout of the same text and style rather than shaping it again; glyph keys include
//! the run's variation coordinates, so one variable font serves every weight.

use std::collections::HashMap;
use std::hash::Hash;
use crate::runtime::{HashMap as FxHashMap, HashSet as FxHashSet};
use std::rc::Rc;
use std::sync::Arc;

use parley::fontique::{Blob, Collection, CollectionOptions, FallbackKey, FamilyId, Script, SourceCache};
use parley::{
    Alignment, AlignmentOptions, FontContext, FontFamily, FontFamilyName, FontStyle, FontWeight, Layout, LayoutContext,
    LineHeight, PositionedLayoutItem, StyleProperty,
};
use swash::FontRef;
use swash::scale::image::Content;
use swash::scale::{Render, ScaleContext, Source, StrikeWith};

use crate::color::Color;
use crate::tok::{Family, OpticalSize, TextStyle};

mod atlas;
mod atlas_fill;
pub(crate) use atlas_fill::QueuedFill;
mod case;
mod color_icon;
mod cursor;
mod decorations;
mod families;
pub(crate) mod full_faces;
mod language;
pub use language::set_language;
mod line_break;
mod span;
mod system_fonts;
mod system_weight;
mod default_fonts;
#[cfg(all(test, target_os = "macos"))]
mod core_text_tests;
pub(crate) mod coretext_match;
pub use coretext_match::FontSmoothing;
#[cfg(not(target_arch = "wasm32"))]
#[doc(hidden)]
pub mod pixel_compare;
mod fallback;
mod glyph_cache;
mod glyph_emit;
mod hash;
mod hidden_axes;
mod inline_fit;
pub(crate) mod icon;
mod layout_cache;
mod macos;
pub use macos::smoothing::ApplePipeline;
mod windows;
mod measure;
pub(crate) mod outline;
pub use outline::Outline;
mod pill_chips;
mod paragraph_join;
mod rich_layout;
mod run;
mod shape;
use shape::{Exact, PillBox, Probe};
pub(crate) use pill_chips::{bare_icons, pill_chips};
mod support;
pub use support::SmoothingSupport;
pub(crate) use support::note_dual_source;
mod svg_path;
mod synthetic_bold;
mod trak;
mod underline;
mod line_metrics;
mod windfoil;
pub use line_metrics::{last_line_fill, layout_height, stream_height_lag};
pub(crate) use line_metrics::spec_height;
pub(crate) use underline::underline_edges;
pub use case::TextCase;
pub use hash::text_hash;
pub(crate) use hash::{directed, elide_key, shape_key};
pub(crate) use families::choose_families;
pub use measure::{FamiliesGuard, TextInk, advance, content_widths, font_epoch, ink, measure, measure_with, with_families};
pub use rich_layout::RichLayoutOptions;
pub use run::{Run, RunLayout};
pub(crate) use case::Source as CaseSource;
pub(crate) use layout_cache::CachedElision;

use default_fonts::DEFAULT_FONTS;
use fallback::Route;
use glyph_emit::Face;
use hidden_axes::hidden_axes;
use layout_cache::{Cache, Entry};
#[cfg(test)]
use layout_cache::STALE_BYTES;
pub use icon::{Icon, icon_coverage, set_icon_swap, set_icon_swap_size};
pub(crate) use icon::Facing;
use icon::rasterize_icon;
use trak::{Trak, Traks, font_trak};

pub use atlas::{ATLAS_SIZE, Atlas, AtlasPage, GlyphQuad};
pub(crate) use cursor::{cursor_from_point, cursor_geometry, glyph_boxes, line_edge, move_line, range_boxes};
pub use decorations::{Deco, SPAN_RADIUS};
pub(crate) use decorations::PaintedDeco;
pub use span::{Ink, Link, LinkPaint, Span, TextHighlight};
pub(crate) use span::{BareIcon, PillChip, PillMetrics, flatten_spans};

use decorations::Decoration;
use atlas::{AtlasGlyph, COLOR_GLYPH_BUDGET, ColorGlyph, GlyphKey, GlyphMap, Rasterized};
#[cfg(test)]
use atlas::{MAX_OVERFLOW_PAGES, MAX_STEADY_PAGES};
#[cfg(test)]
pub(crate) use glyph_emit::GLIDE_PHASES;
use line_break::break_paragraphs;
use span::PILL_BOX;
use system_fonts::{
    LoadedFiles, SYMBOL_FAMILIES, SYMBOL_FONT_FILES, family_list, load_families, load_installed, load_system_mono, named_list, names_system_mono,
    register_cjk_fallbacks, register_script_fallbacks,
};
#[cfg(any(test, not(target_arch = "wasm32")))]
use system_fonts::system_font_files;
#[cfg(test)]
use system_fonts::CJK_FONT_FILES;

/// Lean of a synthetic oblique, in degrees, for faces with no italic.
const SYNTHETIC_OBLIQUE: u8 = 12;

/// The colour image of `glyph` (COLR layers or an embedded bitmap scaled to size), or None
/// when the font draws it as an outline.
fn rasterize_color(scaler: &mut swash::scale::Scaler<'_>, glyph: u16) -> Option<(crate::pixels::Pixels, swash::zeno::Placement)> {
    if !scaler.has_color_bitmaps() && !scaler.has_color_outlines() {
        return None;
    }
    let image = Render::new(&[Source::ColorOutline(0), Source::ColorBitmap(StrikeWith::BestFit)]).render(scaler, glyph)?;
    let (w, h) = (image.placement.width, image.placement.height);
    if image.content != Content::Color || w == 0 || h == 0 || image.data.len() != (w * h * 4) as usize {
        return None;
    }
    let pixels = crate::pixels::Pixels::new();
    pixels.set(w, h, image.data);
    Some((pixels, image.placement))
}

// ---------------------------------------------------------------------------------------------

pub struct TextSystem {
    font_cx: FontContext,
    layout_cx: LayoutContext<Ink>,
    scale_cx: ScaleContext,
    layouts: Cache<Rc<Layout<Ink>>>,
    /// The `layouts` key of the latest layout of each shaping (see `layout_blocks_inner`), whose
    /// glyphs a new width re-breaks instead of shaping the text again.
    shapes: HashMap<u64, u64>,
    /// Shapings whose text was edited this commit (see [`Self::supersede`]).
    superseded: FxHashSet<u64>,
    elisions: Cache<CachedElision>,
    runs: Cache<Rc<RunLayout>>,
    /// Long texts' paragraph layouts, which their joined layouts in `layouts` are made of.
    paragraphs: paragraph_join::Paragraphs,
    generation: u32,
    /// The slots' families as named; `sans` and `mono` are the lists text asks for.
    families: (String, String),
    /// The `choose_families` change these slots follow.
    chosen: u64,
    /// The language text shapes in, and the `set_language` change it follows.
    language: Option<parley::fontique::Language>,
    language_epoch: u64,
    /// Whether it swapped in the full faces a web app fetches (see `full_faces`).
    full_faces: bool,
    pub sans: String,
    pub mono: String,
    symbol_families: Vec<&'static str>,
    /// The colour emoji family among `symbol_families`.
    emoji: Option<&'static str>,
    /// The installed math families, once a text has needed them.
    math: Option<Vec<&'static str>>,
    /// The installed `Family::SystemMono` families, once a text has needed them.
    system_mono: Option<Vec<String>>,
    /// The sans or mono slot names `ui-monospace` (see `family_list`).
    slot_system_mono: bool,
    /// Bit `i` is set once `fallback::SCRIPT_FONTS[i]` has been looked up.
    scripts: u64,
    font_files: LoadedFiles,
    /// The loaded system faces' families, for the face DirectWrite picks (`system_weight`).
    system_faces: system_weight::Faces,
    /// Installed families looked up by name (`load_installed`), lowercase.
    installed: std::collections::HashSet<String>,
    pub atlas: Atlas,
    color_glyphs: GlyphMap<ColorGlyph>,
    color_pixels: Vec<crate::pixels::Pixels>,
    color_bytes: usize,
    color_icons: color_icon::ColorIcons,
    /// Layouts shaped, a running count (shaping work counter for perf gates).
    pub shaped: u32,
    /// Layouts re-broken at a new width from an earlier shaping, a running count.
    pub rebroken: u32,
    /// Reused across lines and frames when mapping reveal glyphs back to source bytes.
    cluster_scratch: Vec<(f32, u32)>,
    coords_scratch: Vec<i16>,
    traks: Traks,
    /// The mode glyphs draw in: [`Self::requested`]'s, or its grayscale twin while subpixel
    /// text can't draw ([`FontSmoothing::drawn`]).
    pub(crate) smoothing: FontSmoothing,
    /// The mode the app asked for ([`Self::set_font_smoothing`]).
    requested: FontSmoothing,
    /// The `trak` table of the first font of a family slot at a weight, upright or italic.
    primary_traks: FxHashMap<(Family, u32, bool), Option<Rc<Trak>>>,
    /// The hidden-axis coordinates (`hidden_axes`) of the same first font.
    primary_axes: FxHashMap<(Family, u32, bool), Rc<[parley::FontVariation]>>,
    /// CoreGraphics' glyph pipeline for `smoothing`, and its per-ink state.
    apple: macos::smoothing::Smoothing,
    /// DirectWrite for `FontSmoothing::DirectWrite`, and its per-ink state.
    windows: windows::Rasterizer,
    pub(crate) glyph_cache: glyph_cache::GlyphCache,
    /// Device px `[x, y]` the next glyphs glide from their resting pixel ([`Self::set_glide`]).
    glide: [f32; 2],
    /// How the glyphs placed next draw ([`Self::begin_text`], [`Self::set_emit`]).
    emit: outline::Emit,
    /// Each painted text's raster path (`outline::RasterPath`), by node.
    paths: outline::Paths,
    /// What each face (font, index) is, for drawing it from outlines.
    face_kinds: FxHashMap<(u64, u32), windfoil::FaceKind>,
}

impl Default for TextSystem {
    fn default() -> Self {
        Self::new(&[], ("Inter", "Inter"))
    }
}

impl TextSystem {
    /// `fonts` replaces the built-in Inter faces when non-empty on native. Web keeps its
    /// bundled sans and mono fallbacks, since there are no system fonts to discover.
    /// `families` names the sans and mono slots; others are selectable with `Family::Named`.
    pub fn new(fonts: &[&'static [u8]], families: (&str, &str)) -> Self {
        measure::note_fonts(fonts, families);
        let mut collection = Collection::new(CollectionOptions { shared: false, system_fonts: false });
        let list: &[&'static [u8]] = if fonts.is_empty() { DEFAULT_FONTS } else { fonts };
        let mut registered = Vec::new();
        #[cfg(all(target_arch = "wasm32", feature = "embedded-fonts"))]
        if !fonts.is_empty() {
            for data in DEFAULT_FONTS {
                collection.register_fonts(Blob::new(Arc::new(*data)), None);
            }
        }
        for data in list {
            let mut added: Vec<FamilyId> = collection.register_fonts(Blob::new(Arc::new(*data)), None).into_iter().map(|(id, _)| id).collect();
            added.sort_unstable();
            registered.extend(added);
        }
        let mut font_files = LoadedFiles::new();
        register_cjk_fallbacks(&mut collection, &mut font_files, &registered, families);
        let symbol_families = load_families(&mut collection, SYMBOL_FONT_FILES, SYMBOL_FAMILIES);
        let emoji = symbol_families.iter().copied().find(|f| f.contains("Emoji"));
        #[cfg(target_arch = "wasm32")]
        let families = if fonts.is_empty() { ("Inter", "IBM Plex Mono") } else { families };
        let (sans, mono) = families::slots(families.0, families.1);
        let mut text = TextSystem {
            font_cx: FontContext { collection, source_cache: SourceCache::default() },
            layout_cx: LayoutContext::new(),
            scale_cx: ScaleContext::new(),
            layouts: Default::default(),
            shapes: Default::default(),
            superseded: Default::default(),
            elisions: Default::default(),
            runs: Default::default(),
            paragraphs: paragraph_join::Paragraphs::new(),
            generation: 0,
            families: (families.0.to_owned(), families.1.to_owned()),
            chosen: 0,
            language: None,
            language_epoch: 0,
            full_faces: false,
            sans, mono,
            symbol_families,
            emoji,
            math: None,
            system_mono: None,
            slot_system_mono: names_system_mono(families.0) || names_system_mono(families.1),
            scripts: 0,
            font_files,
            system_faces: Default::default(),
            installed: Default::default(),
            atlas: Atlas::new(),
            color_glyphs: GlyphMap::default(),
            color_pixels: Vec::new(),
            color_bytes: 0,
            color_icons: color_icon::ColorIcons::default(),
            shaped: 0,
            rebroken: 0,
            cluster_scratch: Vec::new(),
            coords_scratch: Vec::new(),
            traks: Traks::new(),
            smoothing: FontSmoothing::default(),
            requested: FontSmoothing::default(),
            primary_traks: Default::default(),
            primary_axes: Default::default(),
            apple: Default::default(),
            windows: Default::default(),
            glyph_cache: Default::default(),
            glide: [0.0; 2],
            emit: outline::Emit::Atlas,
            paths: Default::default(),
            face_kinds: Default::default(),
        };
        text.load_installed(families.0);
        text.load_installed(families.1);
        text.follow_full_faces();
        text
    }

    /// The `trak` table of the font text in `family` at `weight` shapes with first.
    fn primary_trak(&mut self, family: Family, weight: f32, italic: bool) -> Option<Rc<Trak>> {
        let key = (family, weight.to_bits(), italic);
        if let Some(trak) = self.primary_traks.get(&key) {
            return trak.clone();
        }
        let font = self.primary_font(family, weight, italic);
        let trak = font.and_then(|f| font_trak(&mut self.traks, &f.blob, f.index));
        self.primary_traks.insert(key, trak.clone());
        trak
    }

    /// The hidden-axis coordinates (`hidden_axes`) of the font text in `family` at `weight`
    /// shapes with first.
    fn primary_axes(&mut self, family: Family, weight: f32, italic: bool) -> Rc<[parley::FontVariation]> {
        let key = (family, weight.to_bits(), italic);
        if let Some(axes) = self.primary_axes.get(&key) {
            return axes.clone();
        }
        let font = self.primary_font(family, weight, italic);
        let axes: Rc<[parley::FontVariation]> = font
            .and_then(|f| FontRef::from_index(f.blob.as_ref(), f.index as usize).map(|r| hidden_axes(r, weight)))
            .unwrap_or_default()
            .into();
        self.primary_axes.insert(key, axes.clone());
        axes
    }

    /// The ascent and descent per em of the font text in `family` at `weight` shapes with first.
    fn primary_extents(&mut self, family: Family, weight: f32, italic: bool) -> Option<(f32, f32)> {
        let font = self.primary_font(family, weight, italic)?;
        let m = FontRef::from_index(font.blob.as_ref(), font.index as usize)?.metrics(&[]);
        let em = f32::from(m.units_per_em.max(1));
        Some((m.ascent / em, m.descent / em))
    }

    /// The font text in `family` at `weight` shapes with first.
    fn primary_font(&mut self, family: Family, weight: f32, italic: bool) -> Option<parley::fontique::QueryFont> {
        if family == Family::SystemMono || self.slot_system_mono {
            self.system_mono();
        }
        if let Family::Named(list) = family {
            self.load_installed(list);
        }
        let system = self.system_mono.as_deref().unwrap_or_default();
        let named = |list: &str| -> Vec<String> {
            family_list(list, system, &self.sans)
                .into_iter()
                .filter_map(|f| match f { FontFamilyName::Named(name) => Some(name.into_owned()), _ => None })
                .collect()
        };
        let names = match family {
            Family::Sans => named(&self.sans),
            Family::Mono => named(&self.mono),
            Family::Named(list) => named_list(list).into_iter().filter_map(|f| match f { FontFamilyName::Named(name) => Some(name.into_owned()), _ => None }).chain(named(&self.sans)).collect(),
            Family::SystemMono => system.iter().cloned().chain(named(&self.mono)).collect(),
        };
        let mut query = self.font_cx.collection.query(&mut self.font_cx.source_cache);
        query.set_families(names.iter().map(|name| parley::fontique::QueryFamily::Named(name)));
        let style = if italic { FontStyle::Italic } else { FontStyle::Normal };
        query.set_attributes(parley::fontique::Attributes::new(parley::fontique::FontWidth::NORMAL, style, FontWeight::new(weight)));
        query.first_available_font()
    }

    /// The installed `Family::SystemMono` families, loading them the first time.
    /// Loads the installed families CSS family list `list` names that aren't registered, by
    /// name, so a slot or `Family::Named` can be any font on the system.
    pub(crate) fn load_installed(&mut self, list: &str) {
        load_installed(&mut self.font_cx.collection, &mut self.font_files, &mut self.installed, list);
    }

    fn system_mono(&mut self) -> &[String] {
        let (collection, loaded) = (&mut self.font_cx.collection, &mut self.font_files);
        self.system_mono.get_or_insert_with(|| load_system_mono(collection, loaded))
    }

    /// The RGBA image of a colour glyph quad (`GlyphQuad::color`, nonzero).
    pub fn color_glyph(&self, color: u32) -> &crate::pixels::Pixels {
        &self.color_pixels[color as usize - 1]
    }

    /// The colours the next glyphs draw in (`spans` by `Ink`). On macOS they choose each
    /// glyph's dilation and on Windows its gamma table, so glyphs in light and dark inks
    /// rasterize separately.
    pub fn set_glyph_inks(&mut self, element: Color, spans: &[Span]) {
        self.apple.set_inks(element, spans);
        self.windows.set_inks(element, spans);
    }

    /// Whether glyphs rasterise with DirectWrite: `FontSmoothing::DirectWrite` where it exists.
    pub(crate) fn direct_write(&mut self) -> bool {
        matches!(self.smoothing, FontSmoothing::DirectWrite | FontSmoothing::ClearType) && self.windows.available()
    }

    /// How glyphs rasterise and place ([`FontSmoothing`]).
    pub fn font_smoothing(&self) -> FontSmoothing { self.requested }

    /// How far each gliding glyph mask's ink sits below the top of its quad, in device px, keyed
    /// as `blend::apron_key` keys its instance: a DirectWrite or Windfoil mask keeps one box across its y
    /// phases and moves its ink down that box by its phase, so the box moves a whole pixel only
    /// where the phase wraps. The motion gate's raster, which spreads a glyph over its quad,
    /// moves its coverage by this so a glide reads as the quarter-pixel steps it draws.
    pub(crate) fn glyph_shifts(&self) -> std::collections::HashMap<[u32; 3], f32> {
        self.atlas.map.iter()
            .filter(|(k, _)| k.subpy != 0 && k.apple & (glyph_emit::DIRECT_WRITE_KEY | glyph_emit::WINDFOIL_KEY) != 0)
            .filter_map(|(k, a)| a.mask().map(|a| ([(a.x as f32).to_bits(), (a.y as f32).to_bits(), (a.page as f32).to_bits()], k.subpy as f32 / glyph_emit::GLIDE_PHASES as f32)))
            .collect()
    }

    /// The exponent a glyph's coverage is raised to when drawn in premultiplied `fill` on this
    /// platform: 1 (coverage as is) on macOS and iOS, and a browser's text gamma for `Off`
    /// elsewhere.
    pub fn coverage_exponent(&self, fill: [f32; 4]) -> f32 {
        let apple = cfg!(any(target_os = "macos", target_os = "ios"));
        coretext_match::coverage_exponent(self.smoothing, fill, apple, cfg!(target_os = "windows"))
    }

    /// Rasterise and place glyphs in `mode` from the next placement on, with its CoreGraphics
    /// stages ([`FontSmoothing::pipeline`]). Glyphs already rasterised in another mode stay
    /// cached under their own keys.
    pub fn set_font_smoothing(&mut self, mode: FontSmoothing) {
        self.requested = mode;
        self.smoothing = mode.drawn(self.lcd_ok());
        self.apple.set_pipeline(self.smoothing.pipeline());
        self.atlas.lcd = self.smoothing.lcd();
    }

    /// Whether subpixel text draws here now: the renderer draws outlines and blends by channel,
    /// and for ClearType, DirectWrite is there.
    fn lcd_ok(&mut self) -> bool {
        support::lcd_ok(self.atlas.outlines.support())
            && (self.requested != FontSmoothing::ClearType || self.windows.available())
    }

    /// The CoreGraphics stages glyphs rasterize with.
    pub fn apple_pipeline(&self) -> ApplePipeline { self.apple.pipeline() }
    /// Overrides the stages [`Self::set_font_smoothing`] chose, until the next call to it; for
    /// measuring the stages one at a time.
    pub fn set_apple_pipeline(&mut self, pipeline: ApplePipeline) { self.apple.set_pipeline(pipeline); }

    /// Begins a new batch of glyph placements, reclaiming unused atlas storage.
    /// Previously emitted quads must no longer be sampled after this call. Runtime
    /// calls this automatically; direct text/renderer users call it before shaping.
    pub fn begin_frame(&mut self) {
        self.begin_frame_pinned(0);
    }

    pub(crate) fn begin_frame_pinned(&mut self, pinned_mask: u32) {
        // A subpixel mode draws grayscale until the renderer can blend by channel, and after it
        // says it can't.
        if self.requested.lcd() && self.requested.drawn(self.lcd_ok()) != self.smoothing {
            self.set_font_smoothing(self.requested);
        }
        self.atlas.begin_frame(pinned_mask);
        self.paths.begin_frame();
        self.glyph_cache.begin_frame();
        if self.color_bytes > COLOR_GLYPH_BUDGET {
            let before = self.color_glyphs.len();
            self.color_glyphs.retain(|_, c| c.used);
            if self.color_glyphs.len() != before {
                // Only retire scales unused in the last frame. Clearing active emoji
                // would rerasterize the entire visible working set every frame once
                // that working set alone exceeds the soft budget.
                let old = std::mem::take(&mut self.color_pixels);
                self.color_bytes = 0;
                for c in self.color_glyphs.values_mut() {
                    self.color_pixels.push(old[c.index as usize - 1].clone());
                    c.index = self.color_pixels.len() as u32;
                    self.color_bytes += c.w as usize * c.h as usize * 4;
                }
            }
        }
        for c in self.color_glyphs.values_mut() { c.used = false; }
        self.color_icons.begin_frame();
    }

    /// Size in points, rounded up to whole device pixels so layout stays on the pixel grid.
    pub fn measure(&mut self, text: &str, style: TextStyle, scale: f32) -> (f32, f32) {
        let l = self.layout_rich(text, &[], style, scale, None);
        (l.full_width().ceil() / scale, layout_height(&l, style, scale) / scale)
    }

    /// The atlas quad for `icon` at `px` device pixels square, top-left at `origin`.
    pub fn icon_quad(&mut self, icon: &'static Icon, px: u32, origin: [f32; 2]) -> Option<GlyphQuad> {
        self.icon_quad_facing(icon, px, origin, Facing::Upright)
    }

    /// [`Self::icon_quad`], drawn facing `facing` (`El::mirror_in_rtl` mirrors it).
    pub(crate) fn icon_quad_facing(&mut self, icon: &'static Icon, px: u32, origin: [f32; 2], facing: Facing) -> Option<GlyphQuad> {
        let icon = icon::swapped(icon);
        // A mirrored drawing is cached apart, under the size's top bit.
        let key = (icon as *const Icon as usize, px | (u32::from(facing == Facing::Mirrored) << 31));
        let entry = match self.atlas.icons.get(&key) {
            Some(&e) => {
                if let Some(a) = e {
                    self.atlas.touch(a.page, a.w * a.h);
                }
                e
            }
            None => match rasterize_icon(icon, px, facing, &mut self.atlas) {
                Rasterized::Placed(g) => {
                    self.atlas.icons.insert(key, Some(g));
                    Some(g)
                }
                Rasterized::Empty => {
                    self.atlas.icons.insert(key, None);
                    None
                }
                Rasterized::Unplaced => None,
            },
        };
        entry.map(|a| GlyphQuad {
            rect: [origin[0], origin[1], px as f32, px as f32],
            uv: [(a.x + 1) as f32, (a.page as u32 * ATLAS_SIZE + a.y + 1) as f32],
            ink: 0,
            byte: 0,
            color: 0,
            outline: None,
            lcd: false,
        })
    }
}

#[cfg(test)]
#[path = "text/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "text/quality_tests.rs"]
mod quality_tests;

#[cfg(test)]
#[path = "text/rewrap_tests.rs"]
mod rewrap_tests;

#[cfg(test)]
#[path = "text/line_bg_tests.rs"]
mod line_bg_tests;
