//! What each edit changed in a [`TextEdit`]'s text, for an app that keeps ranges of it in step
//! (comments anchored to a passage, find matches, bookmarks): [`TextEdit::report_changes`]
//! turns the record on and [`TextEdit::take_changes`] reads it.
//!
//! Every edit funnels through the text engine (`apply_text_and_attr_replace`), the secure
//! replacement, `replace_text`, or undo and redo, and each reports there. Undo and redo report
//! the exact inverse of the edits they take back: each undo step keeps the changes it was made
//! of (touching ones merged, as a typing run's are), while reporting is on.

use std::ops::Range;
use std::sync::Arc;

use super::TextEdit;

/// One replacement in a [`TextEdit`]'s text: bytes `range` of the text as it was just before
/// this change were replaced by `inserted` bytes, which now sit at
/// `range.start..range.start + inserted`. An insertion has an empty `range`; a deletion inserts
/// nothing. Offsets are on char boundaries. Changes come in the order they happened, each in
/// the text the one before it left.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TextChange {
    pub range: Range<usize>,
    pub inserted: usize,
}

impl TextChange {
    pub fn new(range: Range<usize>, inserted: usize) -> Self {
        TextChange { range, inserted }
    }

    /// The change that takes this one back: the replacement's bytes replaced by as many as
    /// this one removed.
    fn inverse(&self) -> TextChange {
        TextChange { range: self.range.start..self.range.start + self.inserted, inserted: self.range.len() }
    }

    /// This change followed by `next` as one change, when `next` touches what this one
    /// inserted (a typing run, a word typed then deleted); `None` when they're apart.
    fn then(&self, next: &TextChange) -> Option<TextChange> {
        let (a, b, l) = (self.range.start, self.range.end, self.inserted);
        if next.range.start > a + l || next.range.end < a {
            return None;
        }
        // The text both cover, in the text between them, and what it was before the first.
        let (start, end) = (a.min(next.range.start), (a + l).max(next.range.end));
        let before_end = b + end.saturating_sub(a + l);
        Some(TextChange { range: start..before_end, inserted: end - start - next.range.len() + next.inserted })
    }
}

/// The record a field keeps while [`TextEdit::report_changes`] is on. Never compared, so a
/// field's record doesn't make two otherwise equal fields differ. Shared between clones (a
/// view's copy of the field) until one changes it.
#[derive(Clone, Default)]
pub(super) struct ChangeLog(Option<Arc<Log>>);

#[derive(Clone, Default)]
struct Log {
    /// Changes the app hasn't taken yet, oldest first.
    unread: Vec<TextChange>,
    /// Changes since the last undo step was recorded: what the next step is made of.
    journal: Vec<TextChange>,
    /// How many changes went into the journal before `journal[0]`.
    journal_from: u64,
}

impl PartialEq for ChangeLog {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

/// Where the change record stood when a snapshot was taken (`None` while it was off), so the
/// undo step that snapshot starts finds its own changes. Never compared, like [`ChangeLog`].
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Mark(Option<u64>);

impl PartialEq for Mark {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl ChangeLog {
    pub(super) fn mark(&self) -> Mark {
        Mark(self.0.as_ref().map(|l| l.journal_from + l.journal.len() as u64))
    }

    /// An edit replaced `range` with `inserted` bytes. `undoable` puts it in the journal for
    /// its undo step: not on a secure field, which records none.
    pub(super) fn push(&mut self, range: Range<usize>, inserted: usize, undoable: bool) {
        let Some(log) = &mut self.0 else { return };
        if range.is_empty() && inserted == 0 {
            return;
        }
        let log = Arc::make_mut(log);
        let change = TextChange { range, inserted };
        if undoable {
            log.journal.push(change.clone());
        }
        log.unread.push(change);
    }

    /// Undo or redo replayed `changes`, which no undo step takes again.
    fn replayed(&mut self, changes: Vec<TextChange>) {
        let Some(log) = &mut self.0 else { return };
        Arc::make_mut(log).unread.extend(changes.into_iter().filter(|c| !c.range.is_empty() || c.inserted > 0));
    }

    /// The changes made since `mark`, merged where they touch, for the undo step being recorded,
    /// and empties the journal. `None` when the record was off at `mark` or has been since.
    pub(super) fn step_since(&mut self, mark: Mark) -> Option<Vec<TextChange>> {
        let log = Arc::make_mut(self.0.as_mut()?);
        let from = log.journal_from;
        let journal = std::mem::take(&mut log.journal);
        log.journal_from = from + journal.len() as u64;
        let skip = usize::try_from(mark.0?.checked_sub(from)?).ok()?;
        Some(merge(Vec::new(), journal.into_iter().skip(skip)))
    }

    /// The history was dropped: no step will take what the journal holds.
    pub(super) fn forget_steps(&mut self) {
        if let Some(log) = &mut self.0 {
            let log = Arc::make_mut(log);
            log.journal_from += log.journal.len() as u64;
            log.journal.clear();
        }
    }
}

/// `changes` after `into`, each joined to the one before it where they touch.
pub(super) fn merge(mut into: Vec<TextChange>, changes: impl IntoIterator<Item = TextChange>) -> Vec<TextChange> {
    for change in changes {
        match into.last().and_then(|last| last.then(&change)) {
            Some(joined) => *into.last_mut().unwrap() = joined,
            None => into.push(change),
        }
    }
    into
}

/// The one change that turns `old` into `new`, for an undo step recorded while reporting was
/// off: the bytes between their common start and common end.
fn difference(old: &str, new: &str) -> TextChange {
    let mut start = old.bytes().zip(new.bytes()).take_while(|(a, b)| a == b).count();
    while !old.is_char_boundary(start) {
        start -= 1;
    }
    let most = old.len().min(new.len()) - start;
    let mut end = old.bytes().rev().zip(new.bytes().rev()).take(most).take_while(|(a, b)| a == b).count();
    while !old.is_char_boundary(old.len() - end) {
        end -= 1;
    }
    TextChange { range: start..old.len() - end, inserted: new.len() - end - start }
}

impl TextEdit {
    /// Keeps a record of what each edit changes in `text`, for an app that keeps ranges of it in
    /// step: comments anchored to a passage, find matches, bookmarks. Every edit reports, the
    /// user's and the app's alike: typing, deleting, paste, cut, an input method's commit (a
    /// composition isn't in `text` until then), a dropped or dictated text, a spelling fix,
    /// [`insert`](Self::insert), [`replace_text`](Self::replace_text) (the whole text, when it
    /// differs), and [`undo`](Self::undo) and [`redo`](Self::redo), which report the exact inverse
    /// of the edits they take back. Read them with [`take_changes`](Self::take_changes), from
    /// `El::on_input_change` and after the app's own edits. Assigning to the public `text`
    /// directly reports nothing. Off by default, when a field keeps no record.
    ///
    /// ```
    /// # use rune::{TextChange, TextEdit};
    /// let mut e = TextEdit::new("one two").report_changes();
    /// e.set_selection(4, 7);
    /// e.insert("three");
    /// assert_eq!(e.take_changes(), [TextChange::new(4..7, 5)]);
    /// e.undo();
    /// assert_eq!(e.take_changes(), [TextChange::new(4..9, 3)]);
    /// ```
    pub fn report_changes(mut self) -> Self {
        self.set_report_changes(true);
        self
    }

    /// Turns [`report_changes`](Self::report_changes) on or off on a field the app already
    /// holds. Turning it off drops what wasn't taken. Undo steps recorded while it was off
    /// report as the one change between the texts before and after.
    pub fn set_report_changes(&mut self, on: bool) {
        match (on, self.changes.0.is_some()) {
            (true, false) => self.changes.0 = Some(Arc::default()),
            (false, true) => self.changes.0 = None,
            _ => {}
        }
    }

    /// The changes made since the last call, oldest first, each in the text the one before it
    /// left ([`TextChange`]). Empty unless [`report_changes`](Self::report_changes) is on.
    pub fn take_changes(&mut self) -> Vec<TextChange> {
        match &mut self.changes.0 {
            Some(log) if !log.unread.is_empty() => std::mem::take(&mut Arc::make_mut(log).unread),
            _ => Vec::new(),
        }
    }

    /// Reports undo or redo replacing `from` with `to`: `steps` (the forward changes of the
    /// step), taken back when `back`, or else the one change between the texts.
    pub(super) fn report_replay(&mut self, steps: Option<&[TextChange]>, back: bool, from: &str, to: &str) {
        if self.changes.0.is_none() {
            return;
        }
        let changes = match steps {
            Some(steps) if back => steps.iter().rev().map(TextChange::inverse).collect(),
            Some(steps) => steps.to_vec(),
            None => vec![difference(from, to)],
        };
        self.changes.replayed(changes);
    }
}

#[cfg(test)]
#[path = "changes_tests.rs"]
mod tests;
