use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend};
use rustifiedgit::{
    app::{App, Modal, Prompt},
    config::{Config, MAX_HISTORY_PAGE_SIZE, State},
    git::Repository,
    ui,
};
use std::{
    fs,
    path::Path,
    process::Command,
    time::{Duration, Instant},
};

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn wait(app: &mut App) {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        app.poll_workers();
        if !app.loading && !app.history_loading {
            return;
        }
        assert!(Instant::now() < deadline, "timed out: {}", app.notice);
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn fixture(state: State) -> (tempfile::TempDir, App) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir(&root).unwrap();
    git(&root, &["init", "-qb", "main"]);
    for (name, value) in [
        ("user.name", "History Recovery"),
        ("user.email", "history@example.invalid"),
        ("commit.gpgsign", "false"),
        ("core.hooksPath", ".test-hooks"),
    ] {
        git(&root, &["config", name, value]);
    }
    for index in 0..3 {
        git(
            &root,
            &["commit", "--allow-empty", "-qm", &format!("Commit {index}")],
        );
    }
    let mut config = Config::load(Some(dir.path().join("config")), &[]).unwrap();
    config.git.auto_fetch = false;
    config.git.auto_refresh = false;
    let mut app = App::new(Repository::discover(&root).unwrap(), config, state);
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

fn screen(app: &mut App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal.draw(|frame| ui::draw(frame, app)).unwrap();
    let buffer = terminal.backend().buffer();
    (0..24)
        .map(|y| (0..80).map(|x| buffer[(x, y)].symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn million_commit_custom_limit_is_accepted_saved_and_restored() {
    assert_eq!(MAX_HISTORY_PAGE_SIZE, 1_000_000);
    let (_dir, mut app) = fixture(State::default());
    custom(&mut app, "1000000");
    wait(&mut app);
    assert!(app.modal.is_none(), "{}", app.notice);
    assert!(app.history_error.is_none());
    assert_eq!(app.history_page_size(), 1_000_000);
    assert_eq!(app.options.commit_limit, 1_000_000);
    assert_eq!(app.snapshot.commits.len(), 3);
    let saved = State::load(&app.config.directory).unwrap();
    assert_eq!(saved.commit_limit, Some(1_000_000));
    let mut restarted = App::new(app.repo.clone(), app.config.clone(), saved);
    wait(&mut restarted);
    assert!(restarted.modal.is_none());
    assert_eq!(restarted.history_page_size(), 1_000_000);
    assert_eq!(restarted.options.commit_limit, 1_000_000);
}

#[test]
fn over_million_custom_limit_is_rejected_without_persisting_it() {
    let (_dir, mut app) = fixture(State::default());
    let generation = app.workers.history.current();
    let before = app.options.commit_limit;
    custom(&mut app, "1000001");
    assert!(matches!(app.modal, Some(Modal::Message(ref message)) if message.contains("1000000")));
    assert_eq!(app.options.commit_limit, before);
    assert_eq!(app.state.commit_limit, None);
    assert_eq!(app.workers.history.current(), generation);
    assert_eq!(
        State::load(&app.config.directory).unwrap().commit_limit,
        None
    );
}

#[test]
fn oversized_saved_limit_is_corrected_once_and_does_not_reopen_after_refresh() {
    let (_dir, mut app) = fixture(State {
        commit_limit: Some(2_000_000),
        ..State::default()
    });
    assert_eq!(app.history_page_size(), 1_000_000);
    assert_eq!(app.options.commit_limit, 1_000_000);
    assert_eq!(app.state.commit_limit, Some(1_000_000));
    assert!(
        matches!(app.modal, Some(Modal::Message(ref message)) if message.contains("2000000") && message.contains("1000000"))
    );
    let saved = State::load(&app.config.directory).unwrap();
    assert_eq!(saved.commit_limit, Some(1_000_000));
    key(&mut app, KeyCode::Esc);
    let generation = app.workers.history.current();
    for _ in 0..3 {
        app.refresh();
        wait(&mut app);
        assert!(app.modal.is_none());
        assert_eq!(app.workers.history.current(), generation);
    }
    let mut restarted = App::new(app.repo.clone(), app.config.clone(), saved);
    wait(&mut restarted);
    assert!(restarted.modal.is_none());
}

#[test]
fn long_message_scrolls_at_small_terminal_size_and_keeps_controls_visible() {
    let (_dir, mut app) = fixture(State::default());
    let message = (0..80)
        .map(|index| format!("Message row {index:03}: {}", "long content ".repeat(5)))
        .chain(std::iter::once("FINAL_MESSAGE_LINE".into()))
        .collect::<Vec<_>>()
        .join("\n");
    app.error(message);
    let initial = screen(&mut app);
    assert!(initial.contains("Message row 000"));
    assert!(initial.contains("Enter / Esc close"), "{initial}");
    assert!(!initial.contains("FINAL_MESSAGE_LINE"));
    assert!(app.message_max_scroll > app.message_page_size);
    key(&mut app, KeyCode::Down);
    assert_eq!(app.message_scroll, 1);
    key(&mut app, KeyCode::Up);
    assert_eq!(app.message_scroll, 0);
    key(&mut app, KeyCode::PageDown);
    let page = app.message_page_size.saturating_sub(1).max(1);
    assert_eq!(app.message_scroll, page);
    key(&mut app, KeyCode::PageUp);
    assert_eq!(app.message_scroll, 0);
    key(&mut app, KeyCode::End);
    assert_eq!(app.message_scroll, app.message_max_scroll);
    let final_page = screen(&mut app);
    assert!(final_page.contains("FINAL_MESSAGE_LINE"), "{final_page}");
    assert!(final_page.contains("Enter / Esc close"), "{final_page}");
    key(&mut app, KeyCode::Home);
    assert_eq!(app.message_scroll, 0);
    key(&mut app, KeyCode::Esc);
    assert!(app.modal.is_none());
    assert_eq!(app.message_scroll, 0);
}
