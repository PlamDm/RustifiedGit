use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use pd_lazygit::{
    app::{App, Modal, Prompt},
    config::{Config, State},
    git::Repository,
    model::Panel,
};
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn wait(app: &mut App) {
    let deadline = Instant::now() + Duration::from_secs(8);
    while app.loading || app.history_loading {
        app.poll_workers();
        assert!(Instant::now() < deadline, "{}", app.notice);
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn fixture(size: usize) -> (tempfile::TempDir, App) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    let config_dir = dir.path().join("config");
    fs::create_dir(&root).unwrap();
    fs::create_dir(&config_dir).unwrap();
    git(&root, &["init", "-qb", "main"]);
    let mut input = String::new();
    for index in 0..400 {
        let message = format!("history commit {index}\n");
        input.push_str(&format!("commit refs/heads/main\nmark :{}\ncommitter History Tester <history@example.invalid> {} +0000\ndata {}\n{}",
            index + 1, 1_700_000_000 + index, message.len(), message));
        if index > 0 {
            input.push_str(&format!("from :{index}\n"));
        }
        input.push('\n');
    }
    let mut child = Command::new("git")
        .args(["fast-import", "--quiet"])
        .current_dir(&root)
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::write(
        config_dir.join("config.yml"),
        format!("git:\n  autoFetch: false\n  commitLimit: {size}\n"),
    )
    .unwrap();
    let config = Config::load(Some(config_dir), &[]).unwrap();
    let mut app = App::new(
        Repository::discover(&root).unwrap(),
        config,
        State::default(),
    );
    wait(&mut app);
    (dir, app)
}

fn key(app: &mut App, code: KeyCode) {
    app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn custom(app: &mut App, value: &str) {
    key(app, KeyCode::Char('L'));
    key(app, KeyCode::Char('6'));
    assert!(matches!(
        app.modal,
        Some(Modal::Input {
            purpose: Prompt::HistoryLimit,
            ..
        })
    ));
    app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    app.paste(value.to_owned());
    key(app, KeyCode::Enter);
}

#[test]
fn graph_action_menu_changes_and_persists_initial_page_size() {
    let (_dir, mut app) = fixture(120);
    assert_eq!(app.snapshot.commits.len(), 120);
    assert_eq!(app.history_page_size(), 120);
    app.select_graph_row(app.commit_graph_row(0));
    let selected = app.selected_commit().unwrap().oid.clone();
    let status_generation = app.workers.refresh.current();
    key(&mut app, KeyCode::Char('o'));
    let Some(Modal::Actions { items, .. }) = &app.modal else {
        panic!("missing graph actions");
    };
    let action = items
        .iter()
        .find(|action| action.label == "Loaded commits…")
        .unwrap();
    assert_eq!(action.key, "L");
    assert!(action.description.contains("120 commits loaded"));
    key(&mut app, KeyCode::Char('L'));
    let Some(Modal::Menu { title, .. }) = &app.modal else {
        panic!("missing history menu");
    };
    assert!(
        title.contains("loaded 120")
            && title.contains("target 120")
            && title.contains("initial/page 120")
    );
    key(&mut app, KeyCode::Char('2'));
    wait(&mut app);
    assert_eq!(app.options.commit_limit, 300);
    assert_eq!(app.snapshot.commits.len(), 300);
    assert_eq!(app.selected_commit().unwrap().oid, selected);
    assert_eq!(
        app.workers.refresh.current(),
        status_generation,
        "limit changes must not scan the worktree"
    );
    let state = State::load(&app.config.directory).unwrap();
    assert_eq!(state.commit_limit, Some(300));
    let mut restarted = App::new(app.repo.clone(), app.config.clone(), state);
    wait(&mut restarted);
    assert_eq!(restarted.options.commit_limit, 300);
    assert_eq!(restarted.snapshot.commits.len(), 300);
}

#[test]
fn custom_size_shrinks_to_nearest_commit_and_sets_paging_increment() {
    let (_dir, mut app) = fixture(300);
    app.select_graph_row(app.commit_graph_row(200));
    assert!(!app.history_loading);
    custom(&mut app, "100");
    wait(&mut app);
    assert_eq!(app.snapshot.commits.len(), 100);
    assert_eq!(app.selected(Panel::Commits), 99);
    assert_eq!(app.selected_commit().unwrap().subject, "history commit 300");
    key(&mut app, KeyCode::Char('L'));
    let Some(Modal::Menu { items, .. }) = &app.modal else {
        panic!("missing menu");
    };
    assert!(items.last().unwrap().label.contains("Load more now (+100"));
    key(&mut app, KeyCode::Char('7'));
    wait(&mut app);
    assert_eq!(app.snapshot.commits.len(), 200);
    assert_eq!(app.options.commit_limit, 200);
    assert_eq!(app.selected_commit().unwrap().subject, "history commit 300");
    app.select_graph_row(app.commit_graph_row(175));
    assert!(app.history_loading);
    wait(&mut app);
    assert_eq!(app.options.commit_limit, 300);
    assert_eq!(app.snapshot.commits.len(), 300);
    assert_eq!(app.history_page_size(), 100);
}

#[test]
fn zero_custom_limit_is_rejected_without_changing_history_or_saved_state() {
    let (_dir, mut app) = fixture(120);
    let generation = app.workers.history.current();
    custom(&mut app, "0");
    assert!(matches!(app.modal, Some(Modal::Message(ref message)) if message.contains("positive")));
    assert_eq!(app.options.commit_limit, 120);
    assert_eq!(app.workers.history.current(), generation);
    assert_eq!(app.state.commit_limit, None);
}

#[test]
fn commit_limit_configuration_is_positive_and_older_state_still_loads() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("config.yml"), "git:\n  commitLimit: 0\n").unwrap();
    let error = Config::load(Some(dir.path().to_path_buf()), &[]).unwrap_err();
    assert!(error.to_string().contains("commitLimit"));
    fs::write(dir.path().join("rust-state.json"), "{\"showIgnored\":true}").unwrap();
    let state = State::load(dir.path()).unwrap();
    assert!(state.show_ignored);
    assert_eq!(state.commit_limit, None);
}

#[test]
fn headless_snapshot_respects_configured_initial_commit_limit() {
    let (dir, app) = fixture(120);
    let cli_config = dir.path().join("cli-config");
    fs::create_dir(&cli_config).unwrap();
    fs::write(cli_config.join("config.yml"), "git:\n  commitLimit: 7\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_pd-lazygit"))
        .arg("-p")
        .arg(&app.repo.root)
        .arg("--use-config-dir")
        .arg(cli_config)
        .arg("--snapshot")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let snapshot: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(snapshot["commits"].as_array().unwrap().len(), 7);
}
