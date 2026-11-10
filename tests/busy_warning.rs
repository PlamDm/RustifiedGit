use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend};
use rustifiedgit::{
    app::{App, Modal},
    config::{Config, State},
    git::{Operation, Repository},
    model::Panel,
};
use std::{
    fs,
    path::Path,
    process::Command,
    time::{Duration, Instant},
};

struct Fixture {
    _directory: tempfile::TempDir,
    app: App,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some((cancel, _, _)) = &self.app.busy {
            cancel.cancel();
        }
    }
}

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

fn wait(app: &mut App, done: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        app.poll_workers();
        if done(app) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "worker timed out: {}",
            app.notice
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn fixture() -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("repo");
    fs::create_dir(&root).unwrap();
    git(&root, &["init", "-qb", "main"]);
    for (name, value) in [
        ("user.name", "Busy Tests"),
        ("user.email", "busy@example.invalid"),
        ("commit.gpgsign", "false"),
        ("core.hooksPath", ".test-hooks"),
    ] {
        git(&root, &["config", name, value]);
    }
    fs::write(root.join("work.txt"), "committed contents\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "base"]);
    fs::write(root.join("work.txt"), "uncommitted contents\n").unwrap();
    let mut config = Config::load(Some(directory.path().join("config")), &[]).unwrap();
    config.git.auto_fetch = false;
    config.git.auto_refresh = false;
    let mut app = App::new(
        Repository::discover(&root).unwrap(),
        config,
        State::default(),
    );
    wait(&mut app, |app| !app.loading && !app.history_loading);
    app.set_focus(Panel::Files);
    Fixture {
        _directory: directory,
        app,
    }
}

fn start_blocker(app: &mut App, command: &str) {
    let mut operation = Operation::shell(command.into());
    operation.label = "Sleeping command".into();
    app.run_operation(operation);
    wait(app, |app| {
        app.output.iter().any(|line| line == "BUSY_READY")
    });
    assert!(app.busy.is_some());
}

fn key(app: &mut App, code: KeyCode) {
    app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn cancel(app: &mut App) {
    app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert!(!app.quit, "Cancelling an active command must not quit");
    wait(app, |app| app.busy.is_none());
}

#[test]
fn busy_popup_names_commands_and_rejected_operation_is_never_queued() {
    let mut fixture = fixture();
    let app = &mut fixture.app;
    start_blocker(app, "printf 'BUSY_READY\\n'; sleep 30");
    app.run_operation(Operation::shell("touch second-command-ran".into()));
    match app.modal.as_ref() {
        Some(Modal::Busy { running, requested }) => {
            assert!(running.contains("Sleeping command"));
            assert!(requested.contains("second-command-ran"));
        }
        _ => panic!("A blocked command must produce a busy popup"),
    }
    assert!(
        app.output
            .iter()
            .any(|line| line.contains("second-command-ran")),
        "The rejected command should also be identified in the command log"
    );
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal
        .draw(|frame| rustifiedgit::ui::draw(frame, app))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let screen = (0..24)
        .map(|y| (0..80).map(|x| buffer[(x, y)].symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(screen.contains("Sleeping command"), "{screen}");
    assert!(screen.contains("Ctrl+c"), "{screen}");
    key(app, KeyCode::Enter);
    assert!(app.modal.is_none());
    assert!(app.busy.is_some());
    assert!(!app.repo.root.join("second-command-ran").exists());
    cancel(app);
    assert!(!app.repo.root.join("second-command-ran").exists());
}

#[test]
fn discard_while_busy_warns_before_confirmation_and_ctrl_c_cancels_from_popup() {
    let mut fixture = fixture();
    let app = &mut fixture.app;
    start_blocker(app, "printf 'BUSY_READY\\n'; sleep 30");
    key(app, KeyCode::Char('d'));
    match app.modal.as_ref() {
        Some(Modal::Busy { requested, .. }) => assert!(requested.contains("Discard")),
        _ => panic!("Discard must show the busy popup before presenting confirmation"),
    }
    assert_eq!(
        fs::read_to_string(app.repo.root.join("work.txt")).unwrap(),
        "uncommitted contents\n"
    );
    cancel(app);
    assert!(app.modal.is_none());
    assert!(app.output.iter().any(|line| line.contains("Cancelled")));
    assert_eq!(
        fs::read_to_string(app.repo.root.join("work.txt")).unwrap(),
        "uncommitted contents\n"
    );
}

#[test]
fn completed_command_does_not_turn_ctrl_c_on_stale_busy_popup_into_quit() {
    let mut fixture = fixture();
    let app = &mut fixture.app;
    start_blocker(
        app,
        "printf 'BUSY_READY\\n'; while ! test -f release-command; do sleep 0.02; done",
    );
    app.run_operation(Operation::shell("touch rejected-after-completion".into()));
    assert!(matches!(app.modal, Some(Modal::Busy { .. })));
    fs::write(app.repo.root.join("release-command"), "release").unwrap();
    wait(app, |app| app.busy.is_none());
    assert!(matches!(app.modal, Some(Modal::Busy { .. })));
    app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert!(!app.quit);
    assert!(!app.repo.root.join("rejected-after-completion").exists());
    key(app, KeyCode::Enter);
    assert!(app.modal.is_none());
}
