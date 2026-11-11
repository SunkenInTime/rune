//! How a text field or selectable text paints its selection
//! ([`El::selection_style`](crate::El::selection_style)): the highlight under the selected text
//! and the ink the selected glyphs take, while the window is active and while it isn't, as a
//! browser paints `::selection`, and optionally while a text field doesn't have the keyboard.

use crate::color::Color;

/// Selection colours: active while the window has focus, inactive while it doesn't.
/// `None` ink keeps the text's own colours.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SelectionStyle {
    active: Color,
    active_ink: Option<Color>,
    inactive: Color,
    inactive_ink: Option<Color>,
    /// [`SelectionStyle::unfocused`]: a field's selection while another element has the keyboard.
    unfocused: Option<(Color, Option<Color>)>,
}

impl SelectionStyle {
    /// Chromium's default selection on Windows and Linux in either color scheme: its
    /// `kDefaultActiveSelectionBgColor` (#1967D2) and `kDefaultInactiveSelectionBgColor`
    /// (#C8C8C8) made see-through by `Color::BlendWithWhite` (alpha 0.8, so they look the same
    /// over white), with white text, and #323232 text while the window is inactive.
    pub fn chromium() -> Self {
        let c = |h: &str| crate::color::hex(h).unwrap_or(Color::TRANSPARENT);
        SelectionStyle { active: c("#0041c6cc"), active_ink: Some(Color::WHITE), inactive: c("#bababacc"), inactive_ink: Some(c("#323232")), unfocused: None }
    }

    /// `highlight` under the selected text, active or not, the text keeping its colours.
    pub const fn new(highlight: Color) -> Self {
        SelectionStyle { active: highlight, active_ink: None, inactive: highlight, inactive_ink: None, unfocused: None }
    }

    /// The selected glyphs take `ink` while the window is active.
    pub const fn ink(mut self, ink: Color) -> Self {
        self.active_ink = Some(ink);
        self
    }

    /// While the window is inactive, `highlight` under the selected text and `ink`, if any, on it.
    pub const fn inactive(mut self, highlight: Color, ink: Option<Color>) -> Self {
        self.inactive = highlight;
        self.inactive_ink = ink;
        self
    }

    /// A text field keeps drawing its selection while it doesn't have the keyboard (another
    /// field, a popover's field or a button took focus), with `highlight` under the selected
    /// text and `ink`, if any, on it, as AppKit draws a text view's selection in grey once it
    /// stops being first responder. That holds whether the window is active or not. Without
    /// it a field draws no selection while it isn't focused, as before; the selection itself
    /// is kept either way and shows again in the active pair when the field takes focus back.
    /// Only text fields: selectable text has no keyboard focus of its own.
    pub const fn unfocused(mut self, highlight: Color, ink: Option<Color>) -> Self {
        self.unfocused = Some((highlight, ink));
        self
    }

    /// The highlight and ink for a field that doesn't have the keyboard, if it draws one.
    pub(crate) fn pick_unfocused(&self) -> Option<(Color, Option<Color>)> { self.unfocused }

    /// The highlight and ink for a window that is `active`.
    pub(crate) fn pick(&self, active: bool) -> (Color, Option<Color>) {
        if active { (self.active, self.active_ink) } else { (self.inactive, self.inactive_ink) }
    }
}

#[cfg(test)]
#[path = "selection_style_tests.rs"]
mod tests;
