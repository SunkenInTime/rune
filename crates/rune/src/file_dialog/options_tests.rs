//! `FileDialogOptions::prompt` and `message`: carried to the host and shown in `--tree`.
use super::*;
use crate::{Cx, El, Env, Runtime, el};

#[test]
fn prompt_and_message_reach_the_dialog_and_tree_only_when_set() {
    let options = FileDialogOptions { title: "Open Folder".into(), ..Default::default() }
        .prompt("Open Folder")
        .message("Choose the folder that holds your drafts.");
    assert_eq!(options.prompt.as_deref(), Some("Open Folder"));
    let mut rt = Runtime::new((), |_: &(), _: &mut Cx<()>| -> El { el() }, (100.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(move |_, tx| tx.open_folder(options, |_: &mut (), _, _| {}));
    assert_eq!(
        rt.file_dialog_json().unwrap(),
        serde_json::json!({ "kind": "folder", "title": "Open Folder", "filters": [], "prompt": "Open Folder", "message": "Choose the folder that holds your drafts." })
    );
    let (_, _, taken) = rt.take_file_dialog().expect("the host takes it");
    assert_eq!((taken.prompt.as_deref(), taken.message.as_deref()), (Some("Open Folder"), Some("Choose the folder that holds your drafts.")));
    rt.cancel_file_dialogs();

    rt.update(|_, tx| tx.open_file(FileDialogOptions::default(), |_: &mut (), _, _| {}));
    let json = rt.file_dialog_json().unwrap();
    assert!(json.get("prompt").is_none() && json.get("message").is_none(), "unset options leave --tree as it was: {json}");
}
