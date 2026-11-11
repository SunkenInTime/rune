# Vendored crates

`parley` and `parley_engine` 0.12.0, as published on crates.io, with two additions so Rune can
lay out a long multi-paragraph text one paragraph at a time and join the paragraphs into one
`parley::Layout` (`crates/rune/src/text/paragraph_join.rs`):

- `parley_engine::ShapedText::append` (`parley_engine/src/shape/shaped_text.rs`): appends the
  shaping of a following text, shifting its byte, character, cluster, glyph and variation
  coordinate indices, mapping its style indices and sharing equal fonts.
- `parley::Layout::join_begin`, `join_part` and `join_end` (`parley/src/layout/join.rs`):
  build a layout from the layouts of consecutive paragraphs, each already broken into lines,
  keeping a prefix of each one's lines moved down by an offset, mapping brushes and inline box
  ids, and measuring the joined width and height as line breaking does.

Nothing else differs from the published sources; `vendor/parley/Cargo.toml` points
`parley_engine` at this copy. To move to a newer Parley, copy the new release here and carry
the two additions over (the first commit that added this folder holds the untouched sources).
