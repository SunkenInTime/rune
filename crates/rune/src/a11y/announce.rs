//! Screen-reader announcements ([`Tx::announce`]): text read out without moving focus, as a
//! status line flashing "Saved". The tree carries it as a live region, a label under the root
//! with `Live::Polite`; each announcement is a new node (two ids taken in turn), which every
//! AccessKit adapter announces when it's added: VoiceOver through
//! `NSAccessibilityAnnouncementRequestedNotification`, Narrator and NVDA through UI
//! Automation's `LiveRegionChanged`, Orca through AT-SPI's `object:announcement`. It stays in
//! the tree until the next one, so a reader that fetches the region's name after the event
//! finds it.
use accesskit::{Live, Node as AkNode, NodeId as AkId, Role};

use super::*;

/// The two ids announcements take in turn: a repeat of the same text is a new node too.
const IDS: [AkId; 2] = [AkId(3), AkId(4)];

/// The last announcement, and how many there have been.
#[derive(Default)]
pub(crate) struct Announced {
    count: u64,
    text: Option<String>,
}

impl Tx {
    /// Asks screen readers to read `text` once this handler's turn is done, without moving
    /// focus or showing anything: a status flash ("Saved", "3 replaced"), a refused action's
    /// reason. Politely, after what the reader is saying. Read on macOS (VoiceOver), Windows
    /// (Narrator, NVDA, JAWS) and Linux (Orca, where its AT-SPI handles announcements) while a
    /// reader has the window's tree; with none listening it does nothing, and a reader that
    /// starts later doesn't hear old ones. No effect on the web, in an embedded view, or in a
    /// build without the `accessibility` feature. Several in one turn: the last is read. Empty
    /// text is ignored. Headless runs keep the last in `--tree`'s `accessibility` tree (a
    /// `live-polite` label) and record `announce` in `--trace`; tests read
    /// `Runtime::announcement`.
    pub fn announce(&mut self, text: impl Into<String>) {
        let text = text.into();
        if !text.is_empty() { self.announce = Some(text); }
    }
}

impl<S: 'static> Runtime<S> {
    /// The last text [`Tx::announce`] asked screen readers to read, while the tree still holds it.
    pub fn announcement(&self) -> Option<&str> { self.announced.text.as_deref() }

    pub(crate) fn announce(&mut self, text: String) {
        self.trace_event("announce");
        self.announced.count += 1;
        self.announced.text = Some(text);
    }

    /// A reader that just started doesn't hear what was announced before it listened.
    pub(crate) fn a11y_forget_announcement(&mut self) { self.announced.text = None; }

    /// What the tree's fingerprint reads of the announcement.
    pub(super) fn a11y_announce_print(&self) -> u64 { if self.announced.text.is_some() { self.announced.count } else { 0 } }

    /// The announcement's node, under the root.
    pub(super) fn a11y_announcement(&self) -> Option<(AkId, AkNode)> {
        let text = self.announced.text.as_deref()?;
        let mut node = AkNode::new(Role::Label);
        // Readers name a label by its value on macOS and by its label elsewhere.
        node.set_label(text);
        node.set_value(text);
        node.set_live(Live::Polite);
        Some((IDS[(self.announced.count % 2) as usize], node))
    }
}

#[cfg(test)]
#[path = "announce_tests.rs"]
mod tests;
