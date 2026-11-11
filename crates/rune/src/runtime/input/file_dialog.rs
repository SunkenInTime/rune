use super::*;
use crate::file_dialog::{DialogState, FileDialogError, FileDialogKind, FileDialogOptions, Pending, Picked, Request};

impl<S: 'static> Runtime<S> {
    /// The file dialog showing, or about to show, from [`Tx::open_file`], [`Tx::save_file`] or
    /// [`Tx::open_folder`].
    pub fn file_dialog(&self) -> Option<(FileDialogKind, &FileDialogOptions)> {
        self.file_dialog.pending().map(|d| (d.kind, &d.options))
    }

    /// Answers the file dialog showing as if the user chose `path`, or cancelled with `None`.
    /// Its callback runs on the next frame. For tests and headless drivers; native dialogs
    /// answer themselves.
    pub fn resolve_file_dialog(&mut self, path: Option<std::path::PathBuf>) -> Result<(), FileDialogError> {
        self.resolve_file_dialog_paths(path.into_iter().collect())
    }

    /// Whether the file dialog showing picks several files ([`Tx::open_files`]).
    pub fn file_dialog_multiple(&self) -> bool { self.file_dialog.pending().is_some_and(|d| d.many) }

    /// [`Runtime::resolve_file_dialog`] with every file chosen; empty cancels. Only a dialog
    /// from [`Tx::open_files`] takes more than one.
    pub fn resolve_file_dialog_paths(&mut self, paths: Vec<std::path::PathBuf>) -> Result<(), FileDialogError> {
        let open = self.file_dialog.pending().ok_or_else(|| FileDialogError("no file dialog is open".into()))?;
        if paths.len() > 1 && !open.many {
            return Err(FileDialogError("this file dialog picks one file".into()));
        }
        let id = open.id;
        self.answer_file_dialog(id, Ok(paths));
        Ok(())
    }

    /// Takes a handler's request. One dialog at a time: a request while one is open is
    /// answered with an error rather than queued, so a double click opens one dialog.
    pub(crate) fn request_file_dialog(&mut self, request: Request) {
        self.next_file_dialog += 1;
        let id = self.next_file_dialog;
        self.file_dialog_answers.push((id, request.on_answer));
        if self.native_overlay {
            self.trace_event("file-dialog:overlay");
            self.queue.push(Action::FileDialog(id, Err(FileDialogError("overlays cannot open file dialogs".into()))));
            return;
        }
        if self.file_dialog.is_open() {
            self.trace_event("file-dialog:busy");
            self.queue.push(Action::FileDialog(id, Err(FileDialogError("a file dialog is already open".into()))));
            return;
        }
        self.trace_event(format_args!("file-dialog:open:{:?}", request.kind));
        self.file_dialog = DialogState::Requested(Pending { id, kind: request.kind, many: request.many, options: request.options });
    }

    /// The dialog for the host to show, marked shown. Nothing while it shows.
    pub(crate) fn take_file_dialog(&mut self) -> Option<(u64, FileDialogKind, FileDialogOptions)> {
        let d = self.file_dialog.show()?;
        Some((d.id, d.kind, d.options.clone()))
    }

    /// Queues dialog `id`'s answer for the next frame, closing it.
    pub(crate) fn answer_file_dialog(&mut self, id: u64, result: Picked) {
        self.file_dialog.answered(id);
        self.queue.push(Action::FileDialog(id, result));
    }

    /// Answers a dialog this host can't show with `why`.
    pub(crate) fn refuse_file_dialog(&mut self, why: &str) {
        if let Some((id, ..)) = self.take_file_dialog() {
            self.answer_file_dialog(id, Err(FileDialogError(why.into())));
        }
    }

    pub(super) fn run_file_dialog(&mut self, id: u64, result: Picked, one: &mut Tx) -> bool {
        let Some(i) = self.file_dialog_answers.iter().position(|(dialog, _)| *dialog == id) else { return false };
        let (_, f) = self.file_dialog_answers.remove(i);
        match &result {
            Ok(paths) if paths.len() > 1 => self.trace_event(format_args!("file-dialog:answer:paths:{}", paths.len())),
            Ok(paths) => self.trace_event(format_args!("file-dialog:answer:{}", if paths.is_empty() { "cancel" } else { "path" })),
            Err(_) => self.trace_event("file-dialog:answer:error"),
        }
        f(states!(self), result, one);
        true
    }

    /// Answers the open or waiting dialog with `Ok(None)`, and runs any queued file dialog
    /// callbacks now while the window's state is still alive. A callback that asks for another
    /// dialog is answered too, up to a bound against one that always asks again.
    pub(crate) fn cancel_file_dialogs(&mut self) {
        for _ in 0..64 {
            if let Some(d) = self.file_dialog.take() {
                self.queue.push(Action::FileDialog(d.id, Ok(Vec::new())));
            }
            let Some(pos) = self.queue.iter().position(|a| matches!(a, Action::FileDialog(..))) else { return };
            let Action::FileDialog(id, result) = self.queue.remove(pos) else { unreachable!() };
            let mut one = self.transaction();
            self.run_file_dialog(id, result, &mut one);
            self.latch_requests(&mut one);
            merge(&mut self.pending, one);
            self.dirty = true;
        }
        eprintln!(
            "rune: file dialog callbacks kept asking for dialogs while their window went away; dropping {}",
            self.file_dialog_answers.len()
        );
        self.file_dialog = DialogState::Closed;
        self.file_dialog_answers.clear();
        self.queue.retain(|a| !matches!(a, Action::FileDialog(..)));
    }

    pub(crate) fn file_dialog_json(&self) -> Option<serde_json::Value> {
        self.file_dialog.pending().map(Pending::to_json)
    }
}
