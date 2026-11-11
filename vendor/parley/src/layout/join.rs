// Copyright 2026 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Joining layouts of consecutive paragraphs into one layout, as if their text had been laid
//! out together. Added for Rune's per-paragraph layout (see `vendor/README.md`).

use alloc::vec::Vec;

use super::data::{AlignedSubtreeOffset, LayoutItem, LineData, LineItemData};
use super::{Decoration, Layout, LayoutItemKind, Style};
use crate::inline_box::{InlineBox, LayoutInlineBox};
use crate::style::Brush;

impl<B: Brush> Layout<B> {
    /// Empties this layout to join paragraph layouts into with [`Self::join_part`], keeping its
    /// buffers. `first` gives the scale, quantization and bidi base level.
    pub fn join_begin(&mut self, first: &Self) {
        self.data.clear();
        self.data.scale = first.data.scale;
        self.data.quantize = first.data.quantize;
        self.data.base_level = first.data.base_level;
        self.data.layout_max_advance = first.data.layout_max_advance;
    }

    /// Appends `part`, a layout of the text that follows what was joined so far, broken into
    /// lines. Only its first `lines` lines are kept, moved down by `dy`; `char_offset` is the
    /// number of characters joined before it. Brushes map through `brush` and inline box ids through
    /// `box_id`. Returns false, appending nothing, when the joined layout would have more styles
    /// than a `u16` indexes.
    ///
    /// The parts must have been built and broken with the same settings (scale, quantization,
    /// base level, maximum advance), each from text that ends in a forced line break except
    /// the last; the joined layout's lines are then those of the whole text broken at the same
    /// forced breaks, provided shaping and bidi resolution did not cross them.
    pub fn join_part(
        &mut self,
        part: &Self,
        char_offset: usize,
        lines: usize,
        dy: f32,
        brush: &mut dyn FnMut(&B) -> B,
        box_id: &mut dyn FnMut(u64) -> u64,
    ) -> bool {
        let d = &mut self.data;
        let p = &part.data;
        // The part's root style is the joined root when they agree, as they do for paragraphs
        // of one text.
        let share_root = !d.styles.is_empty()
            && p.styles.first().is_some_and(|s| remap_style(s, &[0], &mut *brush) == d.styles[0])
            && p.style_metrics.first() == d.style_metrics.first();
        let base = d.styles.len();
        let added = p.styles.len() - usize::from(share_root);
        if base + added > usize::from(u16::MAX) + 1 {
            return false;
        }
        #[allow(clippy::cast_possible_truncation, reason = "checked against u16::MAX above")]
        let map: Vec<u16> = (0..p.styles.len())
            .map(|i| match (share_root, i) {
                (true, 0) => 0,
                (true, i) => (base + i - 1) as u16,
                (false, i) => (base + i) as u16,
            })
            .collect();

        let bytes = d.text_len;
        let runs = d.runs.len();
        let boxes = d.inline_boxes.len();
        let clusters = d.shaped_text.shaped_clusters().len() as u32;
        let line_items = d.line_items.len();
        #[allow(clippy::cast_possible_truncation, reason = "offsets stay within u32 like Parley's own")]
        let subtrees = d.aligned_subtree_offsets.len() as u32;

        for (i, s) in p.styles.iter().enumerate() {
            if !(share_root && i == 0) {
                d.styles.push(remap_style(s, &map, &mut *brush));
            }
        }
        for (i, m) in p.style_metrics.iter().enumerate() {
            if !(share_root && i == 0) {
                let mut m = *m;
                m.parent = map[usize::from(m.parent)];
                m.aligned_subtree_root = map[usize::from(m.aligned_subtree_root)];
                d.style_metrics.push(m);
            }
        }
        d.inline_boxes.extend(p.inline_boxes.iter().map(|b| LayoutInlineBox {
            inline_box: InlineBox { id: box_id(b.inline_box.id), index: b.inline_box.index + bytes, ..b.inline_box.clone() },
            parent_style_index: map[usize::from(b.parent_style_index)],
            baseline_offset: b.baseline_offset,
        }));
        d.shaped_text.append(&p.shaped_text, bytes, char_offset, &map);
        d.runs.extend_from_slice(&p.runs);
        let index = |kind: LayoutItemKind, i: usize| match kind {
            LayoutItemKind::TextRun => i + runs,
            LayoutItemKind::InlineBox => i + boxes,
        };
        d.items.extend(p.items.iter().map(|it| LayoutItem { index: index(it.kind, it.index), ..it.clone() }));

        let kept = &p.lines[..lines.min(p.lines.len())];
        let items_end = kept.iter().map(|l| l.item_range.end).max().unwrap_or(0);
        let subtrees_end = kept.iter().map(|l| l.aligned_subtree_offsets.end).max().unwrap_or(0);
        d.lines.extend(kept.iter().map(|l| {
            let mut metrics = l.metrics;
            if dy != 0.0 {
                metrics.baseline += dy;
                metrics.block_min_coord += dy;
                metrics.block_max_coord += dy;
                metrics.content_block_min_coord += dy;
                metrics.content_block_max_coord += dy;
            }
            let mut justification = l.justification;
            if justification.justification_end_cluster != u32::MAX {
                justification.justification_end_cluster += clusters;
            }
            LineData {
                text_range: l.text_range.start + bytes..l.text_range.end + bytes,
                justification,
                item_range: l.item_range.start + line_items..l.item_range.end + line_items,
                metrics,
                aligned_subtree_offsets: l.aligned_subtree_offsets.start + subtrees..l.aligned_subtree_offsets.end + subtrees,
                ..l.clone()
            }
        }));
        d.line_items.extend(p.line_items[..items_end].iter().map(|it| LineItemData {
            index: index(it.kind, it.index),
            text_range: it.text_range.start + bytes..it.text_range.end + bytes,
            shaped_cluster_range: it.shaped_cluster_range.start + clusters..it.shaped_cluster_range.end + clusters,
            ..it.clone()
        }));
        d.aligned_subtree_offsets.extend(
            p.aligned_subtree_offsets[..subtrees_end as usize]
                .iter()
                .map(|o| AlignedSubtreeOffset { root: map[usize::from(o.root)], ..*o }),
        );
        d.text_len += p.text_len;
        true
    }

    /// Finishes a join: the width, full width and height, as line breaking measures them.
    pub fn join_end(&mut self) {
        let d = &mut self.data;
        let (mut width, mut full_width, mut height) = (0_f32, 0_f32, 0_f64);
        for line in &d.lines {
            let line_max = line.metrics.inline_min_coord + line.metrics.advance + line.metrics.indent.max(0.0);
            full_width = full_width.max(line_max);
            width = width.max(line_max - line.metrics.hanging_advance);
            height += f64::from(line.metrics.line_height);
        }
        if let Some(last) = d.lines.last()
            && last.item_range.is_empty()
        {
            height -= f64::from(last.metrics.line_height);
        }
        d.width = width;
        d.full_width = full_width;
        #[allow(clippy::cast_possible_truncation, reason = "as line breaking stores it")]
        let height = height as f32;
        d.height = height;
    }
}

fn remap_style<B: Brush>(s: &Style<B>, map: &[u16], brush: &mut dyn FnMut(&B) -> B) -> Style<B> {
    let underline = s.underline.as_ref().map(|d| Decoration { brush: brush(&d.brush), ..d.clone() });
    let strikethrough = s.strikethrough.as_ref().map(|d| Decoration { brush: brush(&d.brush), ..d.clone() });
    Style {
        parent: map.get(usize::from(s.parent)).copied().unwrap_or(0),
        brush: brush(&s.brush),
        underline,
        strikethrough,
        ..s.clone()
    }
}
