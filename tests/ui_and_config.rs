use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend};
use rustifiedgit::{
    app::{App, Modal, render_template},
    config::{Config, State, key_matches},
    git::{Repository, parse_status},
    model::{FileEntry, Panel, Snapshot},
    tree, ui,
    worker::Latest,
};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::Path,
    process::Command,
    sync::{Arc, mpsc},
    time::Duration,
};

#[test]
fn tree_preserves_dimming_collapse_search_and_flat_mode() {
    let files = vec![
        FileEntry::clean("dir/clean".into()),
        FileEntry {
            path: "dir/changed".into(),
            old_path: None,
            index: b' ',
            worktree: b'M',
        },
        FileEntry {
            path: "secret".into(),
            old_path: None,
            index: b'!',
            worktree: b'!',
        },
    ];
    let rows = tree::rows(&files, true, true, &HashSet::new(), "", false);
    assert_eq!(rows.len(), 4);
    assert!(rows[0].directory && rows[0].changed);
    let collapsed = HashSet::from(["dir".into()]);
    assert_eq!(
        tree::rows(&files, true, true, &collapsed, "", false).len(),
        2
    );
    assert_eq!(
        tree::rows(&files, true, true, &collapsed, "cln", false).len(),
        2
    );
    assert_eq!(
        tree::rows(&files, false, false, &HashSet::new(), "", false).len(),
        2
    );
    assert_eq!(
        tree::rows(&files, false, true, &HashSet::new(), "", true).len(),
        2
    );
}

#[test]
fn yaml_merging_preserves_explicit_false_and_key_remaps() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("config.yml"), "gui:\n  showUnchangedFiles: false\n  diffPreviewMaxBytes: 123\nkeybinding:\n  files:\n    toggleIgnoredFiles: [x, '<ctrl+i>']\n").unwrap();
    let extra = dir.path().join("extra.yml");
    fs::write(&extra, "gui:\n  showFileTree: false\n").unwrap();
    let config = Config::load(Some(dir.path().into()), &[extra]).unwrap();
    assert!(!config.gui.show_unchanged_files);
    assert!(!config.gui.show_file_tree);
    assert_eq!(config.gui.diff_preview_max_bytes, 123);
    assert!(config.binding(
        "files",
        "toggleIgnoredFiles",
        &["I"],
        KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE)
    ));
    assert!(!config.binding(
        "files",
        "toggleIgnoredFiles",
        &["I"],
        KeyEvent::new(KeyCode::Char('I'), KeyModifiers::SHIFT)
    ));
    let state = State {
        diff_preview_enabled: false,
        show_ignored: true,
        ..State::default()
    };
    state.save(dir.path()).unwrap();
    let loaded = State::load(dir.path()).unwrap();
    assert!(!loaded.diff_preview_enabled);
    assert!(loaded.show_ignored);
    assert!(dir.path().join("config.yml").exists());
    assert!(!dir.path().join("state.yml").exists());
}

#[test]
fn invalid_configuration_is_reported_instead_of_silently_reset() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("config.yml"), "gui: [broken").unwrap();
    assert!(Config::load(Some(dir.path().into()), &[]).is_err());
    fs::write(dir.path().join("config.yml"), "gui:\n  sidePanelWidth: 5\n").unwrap();
    assert!(Config::load(Some(dir.path().into()), &[]).is_err());
}

#[test]
fn configured_keys_distinguish_case_and_modifiers() {
    assert!(key_matches(
        "I",
        KeyEvent::new(KeyCode::Char('I'), KeyModifiers::SHIFT)
    ));
    assert!(!key_matches(
        "I",
        KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE)
    ));
    assert!(key_matches(
        "<ctrl+t>",
        KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL)
    ));
    assert!(key_matches(
        "<alt+m>",
        KeyEvent::new(KeyCode::Char('m'), KeyModifiers::ALT)
    ));
    assert!(key_matches(
        "<backtab>",
        KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT)
    ));
    assert!(!key_matches(
        "<disabled>",
        KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE)
    ));
}

#[test]
fn custom_templates_support_quoting_and_reject_unimplemented_expressions() {
    let values = BTreeMap::from([(".SelectedFile.Name".into(), "a file; echo unsafe".into())]);
    assert_eq!(
        render_template("git diff -- {{quote .SelectedFile.Name}}", &values).unwrap(),
        "git diff -- 'a file; echo unsafe'"
    );
    assert!(render_template("{{.Unknown}}", &values).is_err());
    assert!(render_template("{{.SelectedFile.Name", &values).is_err());
}

#[test]
fn stale_worker_requests_are_cancelled_and_pending_work_is_coalesced() {
    let (sender, receiver) = mpsc::channel();
    let worker = Latest::spawn(move |_, value: usize, cancel| {
        for _ in 0..20 {
            if cancel.is_cancelled() {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        sender.send(value).unwrap();
    });
    worker.submit(0);
    for index in 1..100 {
        worker.submit(index);
    }
    assert_eq!(receiver.recv_timeout(Duration::from_secs(2)).unwrap(), 99);
    assert!(receiver.try_recv().is_err());
}

fn app() -> (tempfile::TempDir, App) {
    let dir = tempfile::tempdir().unwrap();
    let output = Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let config = Config::load(Some(dir.path().join("config")), &[]).unwrap();
    let app = App::new(
        Repository::discover(dir.path()).unwrap(),
        config,
        State::default(),
    );
    (dir, app)
}

#[test]
fn panel_eight_focuses_command_output_and_restores_hidden_output() {
    let (_dir, mut app) = app();
    app.state.hide_command_log = true;
    app.handle_key(KeyEvent::new(KeyCode::Char('8'), KeyModifiers::NONE));
    assert_eq!(app.focus, Panel::Output);
    assert!(!app.state.hide_command_log);
    let mut terminal = Terminal::new(TestBackend::new(100, 32)).unwrap();
    terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
    let buffer = terminal.backend().buffer();
    let text = format!("{buffer:?}");
    assert!(text.contains("Command output"));
}

#[test]
fn preview_off_is_respected_and_explicit_opening_is_still_available() {
    let (_dir, mut app) = app();
    app.state.diff_preview_enabled = false;
    app.changed_only = false;
    app.snapshot = Arc::new(Snapshot {
        files: vec![FileEntry::clean("file".into())],
        ..Snapshot::default()
    });
    app.rebuild();
    app.request_preview(false);
    assert!(app.preview_lines[0].contains("off"));
    assert!(!app.state.diff_preview_enabled);
    app.handle_key(KeyEvent::new(KeyCode::Char('0'), KeyModifiers::NONE));
    assert_eq!(app.focus, Panel::Diff);
    assert!(app.preview_loading);
    assert!(!app.state.diff_preview_enabled);
}

#[test]
fn ignored_file_cannot_be_staged_through_ui_and_small_screens_do_not_panic() {
    let (_dir, mut app) = app();
    app.snapshot = Arc::new(Snapshot {
        files: vec![FileEntry {
            path: "ignored".into(),
            old_path: None,
            index: b'!',
            worktree: b'!',
        }],
        ..Snapshot::default()
    });
    app.rebuild();
    app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    assert!(matches!(app.modal, Some(Modal::Message(_))));
    assert!(app.busy.is_none());
    for (width, height) in [(1, 1), (35, 8), (36, 12), (80, 24), (160, 50)] {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
    }
}

#[test]
fn malformed_status_is_not_treated_as_clean_repository() {
    assert!(parse_status(b"M\0").is_err());
    assert!(parse_status(b"R  new\0").is_err());
    let (files, _) = parse_status(b"?? a\nname\0").unwrap();
    assert_eq!(files[0].path, Path::new("a\nname"));
}

#[test]
fn diff_search_navigates_matches_without_replacing_explicit_preview() {
    let (_dir, mut app) = app();
    app.focus = Panel::Diff;
    app.state.diff_preview_enabled = false;
    app.preview_lines = vec![
        "header".into(),
        "+needle one".into(),
        "context".into(),
        "+needle two".into(),
    ];
    for character in "/needle".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.diff_line, 1);
    assert_eq!(app.preview_lines.len(), 4);
    app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
    assert_eq!(app.diff_line, 3);
    app.handle_key(KeyEvent::new(KeyCode::Char('N'), KeyModifiers::SHIFT));
    assert_eq!(app.diff_line, 1);
}

#[test]
fn copied_commits_keep_source_order_after_switching_branch_histories() {
    use rustifiedgit::model::Commit;
    let (_dir, mut app) = app();
    app.focus = Panel::Commits;
    app.graph_working_tree = false;
    app.snapshot = Arc::new(Snapshot {
        commits: vec![
            Commit {
                oid: "newer".into(),
                ..Commit::default()
            },
            Commit {
                oid: "older".into(),
                ..Commit::default()
            },
        ]
        .into(),
        ..Snapshot::default()
    });
    app.handle_key(KeyEvent::new(KeyCode::Char('C'), KeyModifiers::SHIFT));
    app.selections[3] = 1;
    app.handle_key(KeyEvent::new(KeyCode::Char('C'), KeyModifiers::SHIFT));
    app.snapshot = Arc::new(Snapshot {
        commits: vec![Commit {
            oid: "target-branch".into(),
            ..Commit::default()
        }]
        .into(),
        ..Snapshot::default()
    });
    app.selections[3] = 0;
    app.handle_key(KeyEvent::new(KeyCode::Char('V'), KeyModifiers::SHIFT));
    let Some(Modal::Confirm { operation, .. }) = &app.modal else {
        panic!("Expected cherry-pick confirmation")
    };
    let rustifiedgit::git::Step::Command(command) = &operation.steps[0] else {
        panic!("Expected Git command")
    };
    assert_eq!(
        command.args,
        ["cherry-pick", "older", "newer"].map(std::ffi::OsString::from)
    );
    assert!(app.marked.is_empty());
}

#[cfg(unix)]
#[test]
fn native_path_bytes_survive_the_parser_and_command_builder() {
    use std::os::unix::ffi::OsStrExt;
    let (files, _) = parse_status(b"?? raw-\xff.txt\0").unwrap();
    assert_eq!(files[0].path.as_os_str().as_bytes(), b"raw-\xff.txt");
    let operation = rustifiedgit::git::stage_operation(&[&files[0]], false, false).unwrap();
    let rustifiedgit::git::Step::Command(command) = &operation.steps[0] else {
        panic!("Expected Git command")
    };
    assert_eq!(command.args.last().unwrap().as_bytes(), b"raw-\xff.txt");
}
