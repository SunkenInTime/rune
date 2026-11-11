//! Open dialogs that pick several files: `Tx::open_files`, `FileDialogs::open_files` and their
//! headless answers.
use super::*;
use crate::{Cx, El, Env, Runtime, el};

#[derive(Default)]
struct Images { picks: Vec<Result<Vec<PathBuf>, String>> }

fn view(_: &Images, _: &mut Cx<Images>) -> El {
    el().key("image").size(80.0, 20.0).on_press_tx(|_: &mut Images, tx: &mut Tx| {
        let options = FileDialogOptions { title: "Insert Image".into(), ..Default::default() }.filter("Images", &["png", "jpg"]);
        tx.open_files(options, |s: &mut Images, result, _| s.picks.push(result.map_err(|e| e.0)));
    })
}

fn press(rt: &mut Runtime<Images>, t: f64) {
    rt.pointer_down(10.0, 10.0);
    rt.pointer_up(10.0, 10.0);
    rt.frame(t);
}

#[test]
fn open_files_waits_open_as_multiple_and_answers_with_every_file_in_order() {
    let mut rt = Runtime::new(Images::default(), view, (200.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    press(&mut rt, 1.0 / 60.0);
    assert_eq!(rt.file_dialog().map(|(kind, o)| (kind, o.title.clone())), Some((FileDialogKind::Open, "Insert Image".into())));
    assert!(rt.file_dialog_multiple());
    assert_eq!(
        rt.file_dialog_json().unwrap(),
        serde_json::json!({ "kind": "open", "title": "Insert Image", "filters": [{ "name": "Images", "extensions": ["png", "jpg"] }], "multiple": true })
    );
    let picked: Vec<PathBuf> = vec!["/pics/b.png".into(), "/pics/a 录.jpg".into(), "/pics/c.png".into()];
    rt.resolve_file_dialog_paths(picked.clone()).unwrap();
    rt.frame(2.0 / 60.0);
    assert_eq!(rt.state.picks, [Ok(picked)], "every file, in the dialog's order");

    press(&mut rt, 3.0 / 60.0);
    rt.resolve_file_dialog(Some("/pics/one.png".into())).unwrap();
    press(&mut rt, 4.0 / 60.0);
    rt.resolve_file_dialog(None).unwrap();
    rt.frame(5.0 / 60.0);
    assert_eq!(rt.state.picks[1..], [Ok(vec!["/pics/one.png".into()]), Ok(vec![])], "one file, then a cancel as an empty list");
}

#[test]
fn a_one_file_dialog_refuses_several_paths_and_is_not_multiple() {
    #[derive(Default)]
    struct One { answers: Vec<FileDialogResult> }
    let mut rt = Runtime::new(One::default(), |_: &One, _: &mut Cx<One>| el(), (100.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|_, tx| tx.open_file(FileDialogOptions::default(), |s: &mut One, r, _| s.answers.push(r)));
    assert!(!rt.file_dialog_multiple());
    assert_eq!(rt.file_dialog_json().unwrap().get("multiple"), None, "--tree is unchanged for one-file dialogs");
    assert!(rt.resolve_file_dialog_paths(vec!["/a".into(), "/b".into()]).is_err());
    assert!(rt.file_dialog().is_some(), "the refused answer leaves it open");
    rt.resolve_file_dialog_paths(vec!["/a".into()]).unwrap();
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.answers, [Ok(Some("/a".into()))]);
}

#[test]
fn closing_cancels_an_open_files_dialog_with_an_empty_list() {
    let mut rt = Runtime::new(Images::default(), view, (200.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    press(&mut rt, 1.0 / 60.0);
    rt.cancel_file_dialogs();
    assert_eq!(rt.state.picks, [Ok(vec![])]);
}

#[test]
fn a_host_answer_and_the_trace_count_the_files() {
    let mut rt = Runtime::new(Images::default(), view, (200.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    press(&mut rt, 1.0 / 60.0);
    let (id, kind, _) = rt.take_file_dialog().expect("the host takes it");
    assert_eq!(kind, FileDialogKind::Open);
    rt.answer_file_dialog(id, Ok(vec!["/a.png".into(), "/b.png".into()]));
    rt.frame(2.0 / 60.0);
    assert_eq!(rt.state.picks, [Ok(vec!["/a.png".into(), "/b.png".into()])]);
}

#[test]
fn scripted_dialogs_answer_open_files_with_one_path_or_with_files() {
    let dialogs = FileDialogs::scripted([
        ScriptedFileDialog { kind: FileDialogKind::Open, path: Some("/one.png".into()) },
        ScriptedFileDialog { kind: FileDialogKind::Open, path: None },
    ])
    .with_files(["/a.png".into(), "/b.png".into()])
    .with_files([]);
    let options = FileDialogOptions::default();
    assert_eq!(pollster::block_on(dialogs.open_files(options.clone())).unwrap(), [PathBuf::from("/one.png")]);
    assert!(pollster::block_on(dialogs.open_files(options.clone())).unwrap().is_empty(), "a cancelled Open fixture");
    assert!(pollster::block_on(dialogs.open(options.clone())).is_err(), "a several-files fixture doesn't answer a one-file Open");
    assert_eq!(pollster::block_on(dialogs.open_files(options.clone())).unwrap(), [PathBuf::from("/a.png"), PathBuf::from("/b.png")]);
    assert!(pollster::block_on(dialogs.open_files(options.clone())).unwrap().is_empty());
    assert!(pollster::block_on(dialogs.open_files(options)).is_err(), "exhausted");
}
