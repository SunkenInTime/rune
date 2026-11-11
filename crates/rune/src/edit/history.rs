//! Undo and redo for a [`TextEdit`]: document snapshots, typing runs coalesced as Chromium's
//! editor does ([`Run`], [`step`]), undo groups and a history bounded in steps and bytes.

use std::borrow::Cow;
use std::collections::VecDeque;

use super::*;

pub(super) const HISTORY_STEPS: usize = 128;
pub(super) const HISTORY_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Snapshot {
    text: Arc<Zeroizing<String>>,
    anchor: usize,
    focus: usize,
    marks: Vec<MarkRange>,
    blocks: Vec<Block>,
    soft_breaks: Vec<u32>,
}

impl Snapshot {
    fn same_doc(&self, other: &Self) -> bool {
        self.text == other.text
            && self.marks == other.marks
            && self.blocks == other.blocks
            && self.soft_breaks == other.soft_breaks
    }
}

/// Whether a [`TextEdit::undo_group`] is open, and what its one step undoes to. Never cloned
/// or compared, so cloning or comparing a [`TextEdit`] mid-group stays clean.
#[derive(Default)]
pub(super) enum Group {
    #[default]
    Closed,
    /// Open on a field that records: the document as the group began, and the step's name.
    Open(Snapshot, Option<StepName>),
    /// Open on a secure field, which records nothing.
    Unrecorded,
}

impl Group {
    pub(super) fn is_open(&self) -> bool {
        !matches!(self, Group::Closed)
    }
}

impl Clone for Group {
    fn clone(&self) -> Self {
        Group::Closed
    }
}

impl PartialEq for Group {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

/// Non-content-derived edit state. Never retain a fingerprint of a secret.
#[derive(Clone, Copy, Default)]
pub(super) struct EditedTag(pub(super) bool);

impl PartialEq for EditedTag {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Change {
    before: Snapshot,
    after: Snapshot,
    name: Option<StepName>,
}

/// What an undo step is called, for an Edit menu's "Undo Typing" ([`TextEdit::undo_name`]).
#[derive(Clone, Debug, PartialEq)]
pub(super) enum StepName {
    /// Typed text, line breaks, deletions and input method commits, as AppKit names them.
    Typing,
    Paste,
    Cut,
    /// The app's name ([`TextEdit::undo_group_named`]).
    Named(Arc<str>),
}

impl StepName {
    /// The name in the app's language: Rune's own through [`crate::set_labels`].
    fn text(&self) -> Cow<'_, str> {
        use crate::labels::{Label, label};
        match self {
            StepName::Typing => Cow::Owned(label(Label::Typing)),
            StepName::Paste => Cow::Owned(label(Label::Paste)),
            StepName::Cut => Cow::Owned(label(Label::Cut)),
            StepName::Named(name) => Cow::Borrowed(name),
        }
    }
}

impl Change {
    fn bytes(&self) -> usize {
        self.before.text.len()
            + self.after.text.len()
            + (self.before.marks.len() + self.after.marks.len()) * std::mem::size_of::<MarkRange>()
            + (self.before.blocks.len() + self.after.blocks.len()) * std::mem::size_of::<Block>()
            + (self.before.soft_breaks.len() + self.after.soft_breaks.len()) * std::mem::size_of::<u32>()
    }
}

// Rendering clones TextEdit. Share immutable history so a view never copies the undo stack.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct History {
    pub(super) undo: VecDeque<Change>,
    pub(super) redo: Vec<Change>,
    pub(super) bytes: usize,
    pub(super) run: Run,
}

/// The undo step the last edit leaves open for the next to join, as a browser's editor keeps
/// its typing command open. A caret move, a pointer selection, a focus change, undo, redo and
/// any edit but typing close it. Pauses and word boundaries don't: Chromium's textarea and
/// contenteditable undo a whole typing run in one step.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Run {
    #[default]
    Closed,
    /// Typed text or line breaks: more join; a deletion starts a new step.
    Inserting,
    /// Backspace or Delete: more deletions and typed text join.
    Deleting,
}

/// What an edit is, for joining it to the open undo step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EditKind {
    /// Typed text without a line break.
    Text,
    /// A typed line break (Return, Shift+Return).
    LineBreak,
    /// Backspace or Delete over a character or a selection.
    Delete,
    /// A word or line deleted at once (Ctrl+Backspace, Ctrl+K): always a new step, which
    /// further deletions and typing join.
    DeleteRun,
    /// Paste, an IME commit, a pill, formatting: a step of its own.
    Other,
}

impl EditKind {
    /// The name of the step an edit of this kind starts: typing, line breaks and deletions are
    /// "Typing", as in AppKit; the rest are named by whoever groups them.
    fn name(self) -> Option<StepName> {
        match self {
            EditKind::Text | EditKind::LineBreak | EditKind::Delete | EditKind::DeleteRun => Some(StepName::Typing),
            EditKind::Other => None,
        }
    }
}

/// Where an edit's change goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Record {
    /// Into the open step.
    Join,
    /// Into a new step.
    Push,
}

/// Steps the open run with an edit of `kind`; `into_empty` when typed text lands in a field
/// that held nothing. The first character typed into an empty field is a step of its own, as
/// in OpenCode's composer (a contenteditable whose first input closes Chromium's typing step),
/// so undoing a typed message leaves that character, then empties the field.
pub(super) fn step(run: &mut Run, kind: EditKind, into_empty: bool) -> Record {
    let (record, next) = match (*run, kind) {
        (_, EditKind::Other) => (Record::Push, Run::Closed),
        (_, EditKind::DeleteRun) | (Run::Closed | Run::Inserting, EditKind::Delete) => (Record::Push, Run::Deleting),
        (Run::Deleting, EditKind::Delete) => (Record::Join, Run::Deleting),
        (Run::Closed, EditKind::Text | EditKind::LineBreak) => (Record::Push, Run::Inserting),
        (Run::Inserting | Run::Deleting, EditKind::Text | EditKind::LineBreak) => (Record::Join, Run::Inserting),
    };
    *run = if kind == EditKind::Text && into_empty { Run::Closed } else { next };
    record
}

impl TextEdit {
    /// Drop the undo and redo stacks, keeping text, marks, blocks, soft breaks and the selection
    /// (after loading a sent message back into a composer to edit it, say).
    pub fn clear_history(&mut self) {
        self.history = Arc::default();
        if self.group.is_open() {
            self.sync_invariants();
            self.group = self.group_from_here();
        }
    }

    /// An undo group opened on the document as it is now, keeping the open group's name.
    pub(super) fn group_from_here(&self) -> Group {
        let name = match &self.group { Group::Open(_, name) => name.clone(), Group::Closed | Group::Unrecorded => None };
        if self.secure { Group::Unrecorded } else { Group::Open(self.snapshot_now(), name) }
    }

    /// Edits record undo steps: not on a secure field, and not inside an undo group, whose
    /// edits become one step when it closes.
    fn recording(&self) -> bool {
        !self.secure && !self.group.is_open()
    }

    /// End a typing group on a pointer selection or focus change.
    pub fn break_history_group(&mut self) {
        if self.history.run != Run::Closed { Arc::make_mut(&mut self.history).run = Run::Closed; }
    }

    pub(super) fn snapshot_now(&self) -> Snapshot {
        Snapshot {
            text: Arc::new(Zeroizing::new(self.text.clone())),
            anchor: self.anchor,
            focus: self.focus,
            marks: self.marks.clone(),
            blocks: self.blocks.clone(),
            soft_breaks: self.soft_breaks.clone(),
        }
    }

    pub(super) fn snapshot(&self) -> Option<Snapshot> {
        self.recording().then(|| self.snapshot_now())
    }

    fn restore(&mut self, s: &Snapshot) {
        self.text.zeroize();
        self.text.push_str(&s.text);
        self.anchor = s.anchor;
        self.focus = s.focus;
        self.marks = s.marks.clone();
        self.blocks = s.blocks.clone();
        self.soft_breaks = s.soft_breaks.clone();
        self.typing = Typing::Inherited;
        self.cancel_composition();
        self.sync_invariants();
    }

    pub(super) fn record_change(&mut self, before: Option<Snapshot>, kind: EditKind) {
        self.record_step(before, kind, kind.name());
    }

    fn record_step(&mut self, before: Option<Snapshot>, kind: EditKind, name: Option<StepName>) {
        let Some(before) = before.filter(|_| self.recording()) else {
            return;
        };
        let after = self.snapshot_now();
        if before.same_doc(&after) {
            return;
        }
        let h = Arc::make_mut(&mut self.history);
        let expected = h.redo.last().map(|c| &c.before).or_else(|| h.undo.back().map(|c| &c.after));
        if expected.is_some_and(|last| last.text != before.text) {
            *h = History::default();
        }
        h.bytes -= h.redo.iter().map(Change::bytes).sum::<usize>();
        h.redo.clear();
        // A rich contenteditable creates its first text node here; a plain field just types.
        let record = step(&mut h.run, kind, self.rich && before.text.is_empty());
        if record == Record::Join
            && before.anchor == before.focus
            && h.undo.back().is_some_and(|c| c.after == before)
        {
            let last = h.undo.back_mut().unwrap();
            h.bytes -= last.bytes();
            last.after = after;
            h.bytes += last.bytes();
        } else {
            let change = Change { before, after, name };
            h.bytes += change.bytes();
            h.undo.push_back(change);
        }
        while h.undo.len() > HISTORY_STEPS || h.bytes > HISTORY_BYTES {
            h.bytes -= h.undo.pop_front().unwrap().bytes();
        }
    }

    /// Run `f`, whose edits become one undo step: one undo reverts all of them and one redo
    /// restores them, with the selection from before the first and after the last. Groups
    /// nest and flatten into the outermost. A group that changes nothing records no step, and
    /// `undo` and `redo` do nothing inside one. Typing after the group starts a new step.
    ///
    /// Picking a mention is one step, trailing space included:
    ///
    /// ```
    /// # use rune::TextEdit;
    /// let mut e = TextEdit::default();
    /// e.insert("hi @ag");
    /// e.undo_group(|e| {
    ///     e.replace_with_pill(3..6, "agent:1", "@agent");
    ///     e.insert(" ");
    /// });
    /// assert_eq!(e.text, "hi @agent ");
    /// e.undo();
    /// assert_eq!(e.text, "hi @ag");
    /// ```
    pub fn undo_group<R>(&mut self, f: impl FnOnce(&mut TextEdit) -> R) -> R {
        self.named_group(None, f)
    }

    /// [`undo_group`](Self::undo_group) whose step is called `name`, which
    /// [`undo_name`](Self::undo_name) and [`redo_name`](Self::redo_name) return for an Edit
    /// menu's "Undo Bold". Inside another group it names that group, unless that group already
    /// has a name.
    ///
    /// ```
    /// # use rune::{Mark, TextEdit};
    /// let mut e = TextEdit::new("bold").rich();
    /// e.set_selection(0, 4);
    /// e.undo_group_named("Bold", |e| e.toggle_mark(Mark::Bold));
    /// assert_eq!(e.undo_name().as_deref(), Some("Bold"));
    /// ```
    pub fn undo_group_named<R>(&mut self, name: impl Into<String>, f: impl FnOnce(&mut TextEdit) -> R) -> R {
        let name: String = name.into();
        self.named_group(Some(StepName::Named(name.into())), f)
    }

    /// Names the open [`undo_group`](Self::undo_group)'s step `name`, replacing any name it
    /// had, as AppKit's `setActionName:` names the open undo group. Outside a group it does
    /// nothing.
    pub fn set_undo_name(&mut self, name: impl Into<String>) {
        let name: String = name.into();
        if let Group::Open(_, slot) = &mut self.group {
            *slot = Some(StepName::Named(name.into()));
        }
    }

    /// Runs `f` as one step called `name`, Rune's own (a paste, a cut): inside an app's group it
    /// names that group only when the app gave it no name.
    pub(super) fn named_step<R>(&mut self, name: StepName, f: impl FnOnce(&mut TextEdit) -> R) -> R {
        self.named_group(Some(name), f)
    }

    fn named_group<R>(&mut self, name: Option<StepName>, f: impl FnOnce(&mut TextEdit) -> R) -> R {
        if self.group.is_open() {
            if let Group::Open(_, slot @ None) = &mut self.group {
                *slot = name;
            }
            return f(self);
        }
        self.sync_invariants();
        self.break_history_group();
        self.group = self.group_from_here();
        if let Group::Open(_, slot) = &mut self.group {
            *slot = name;
        }
        struct Reset<'a>(&'a mut TextEdit);
        impl Drop for Reset<'_> {
            fn drop(&mut self) {
                self.0.group = Group::Closed;
            }
        }
        let g = Reset(self);
        let r = f(g.0);
        let (before, name) = match std::mem::take(&mut g.0.group) {
            Group::Open(baseline, name) => (Some(baseline), name),
            Group::Closed | Group::Unrecorded => (None, None),
        };
        drop(g);
        self.sync_invariants();
        self.break_history_group();
        self.record_step(before, EditKind::Other, name);
        r
    }

    /// The step [`undo`](Self::undo) would take back now, if there is one.
    fn undoable(&self) -> Option<&Change> {
        self.history.undo.back().filter(|c| self.recording() && self.text == c.after.text.as_str())
    }

    /// The step [`redo`](Self::redo) would make again now, if there is one.
    fn redoable(&self) -> Option<&Change> {
        self.history.redo.last().filter(|c| self.recording() && self.text == c.before.text.as_str())
    }

    /// Whether [`undo`](Self::undo) has a step to take back: false with nothing recorded, on a
    /// secure field, inside an [`undo_group`](Self::undo_group), or once the app replaced
    /// `text` under the history.
    pub fn can_undo(&self) -> bool {
        self.undoable().is_some()
    }

    /// Whether [`redo`](Self::redo) has a step to make again.
    pub fn can_redo(&self) -> bool {
        self.redoable().is_some()
    }

    /// The name of the step [`undo`](Self::undo) would take back, for an Edit menu's "Undo
    /// Typing": "Typing" for typed text, line breaks, deletions and input method commits (one
    /// step per typing run), "Paste" and "Cut" for those, and the app's name for an
    /// [`undo_group_named`](Self::undo_group_named) step. Rune's names are its labels, so
    /// [`crate::set_labels`] translates them (`Label::Typing`, `Label::Paste`, `Label::Cut`).
    /// `None` when there's nothing to undo or the step has no name (formatting, a pill, an
    /// unnamed group); [`can_undo`](Self::can_undo) tells those apart.
    pub fn undo_name(&self) -> Option<Cow<'_, str>> {
        self.undoable()?.name.as_ref().map(StepName::text)
    }

    /// The name of the step [`redo`](Self::redo) would make again, as
    /// [`undo_name`](Self::undo_name).
    pub fn redo_name(&self) -> Option<Cow<'_, str>> {
        self.redoable()?.name.as_ref().map(StepName::text)
    }

    /// Undo one typing group or discrete edit. Active IME composition is cancelled first.
    pub fn undo(&mut self) {
        if !self.recording() { return; }
        self.break_history_group();
        if self.preedit().is_some() { self.cancel_composition(); return; }
        let h = Arc::make_mut(&mut self.history);
        let Some(change) = h.undo.pop_back() else { return };
        // Public model text may have been replaced by the app. Never restore stale history.
        if self.text != change.after.text.as_str() { *h = History::default(); return; }
        self.restore(&change.before);
        Arc::make_mut(&mut self.history).redo.push(change);
    }

    pub fn redo(&mut self) {
        if !self.recording() { return; }
        self.break_history_group();
        if self.preedit().is_some() { self.cancel_composition(); return; }
        let h = Arc::make_mut(&mut self.history);
        let Some(change) = h.redo.pop() else { return };
        if self.text != change.before.text.as_str() { *h = History::default(); return; }
        self.restore(&change.after);
        Arc::make_mut(&mut self.history).undo.push_back(change);
    }
}

#[cfg(test)]
#[path = "history_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "undo_names_tests.rs"]
mod names_tests;
