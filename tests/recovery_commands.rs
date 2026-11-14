use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend};
use rustifiedgit::{
    app::{App, Modal},
    config::{Config, State},
    git::{Operation, Repository, Step},
    model::Panel,
    recovery::{MessageSelection, command_line},
};
use std::{
    path::Path,
    process::Command,
    time::{Duration, Instant},
};

fn git(root: &Path, args: &[&str]) -> String {
    let result = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8_lossy(&result.stdout).trim().into()
}
fn wait(app: &mut App) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        app.poll_workers();
        if app.busy.is_none() && !app.loading && !app.history_loading && !app.inspection_loading {
            break;
        }
        assert!(Instant::now() < deadline, "{}", app.notice);
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn fixture() -> (tempfile::TempDir, App) {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-qb", "main"]);
    for (key, value) in [
        ("user.name", "Recovery Test"),
        ("user.email", "recovery@example.invalid"),
        ("commit.gpgsign", "false"),
        ("core.hooksPath", ".test-hooks"),
    ] {
        git(dir.path(), &["config", key, value]);
    }
    git(dir.path(), &["commit", "--allow-empty", "-qm", "base"]);
    git(dir.path(), &["branch", "feature/user/one"]);
    git(dir.path(), &["branch", "feature/user/two"]);
    git(
        dir.path(),
        &["update-ref", "refs/remotes/origin/topic", "HEAD"],
    );
    let mut config = Config::load(Some(dir.path().join("config")), &[]).unwrap();
    config.git.auto_fetch = false;
    config.git.auto_refresh = false;
    let mut app = App::new(
        Repository::discover(dir.path()).unwrap(),
        config,
        State::default(),
    );
    wait(&mut app);
    (dir, app)
}
fn key(app: &mut App, code: KeyCode) {
    app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
}
fn screen(app: &mut App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(62, 16)).unwrap();
    terminal
        .draw(|frame| rustifiedgit::ui::draw(frame, app))
        .unwrap();
    terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

#[test]
fn commands_preserve_literal_arguments_and_shell_expressions_remain_copy_only() {
    let command =
        command_line("hint:   git config --global --add safe.directory '/some repo/[name]'")
            .unwrap();
    assert_eq!(command.program, "git");
    assert_eq!(command.args.last().unwrap(), "/some repo/[name]");
    assert_eq!(
        command_line("$ gpgconf --kill gpg-agent").unwrap().program,
        "gpgconf"
    );
    for line in [
        "git pull && git reset --hard",
        "git config key $(touch sentinel)",
        "git status; echo bad",
        "git show `id`",
        "git switch <branch>",
        "git status > output",
        "git status | cat",
        "git status\nother",
        "git status\x1b[0m",
        "fatal: run git status",
        "git checkout ...",
        "git config --global user.email 'you@example.com'",
        "git config --global user.name 'Your Name'",
    ] {
        assert!(command_line(line).is_none(), "{line}");
    }
}

#[test]
fn copying_and_marking_are_exact_and_generated_fixes_do_not_run_automatically() {
    let (message, mut state) = MessageSelection::prepare(
        "fatal: Need to specify how to reconcile divergent branches.".into(),
    );
    assert_eq!(state.commands.len(), 3);
    assert!(state.operation().is_none());
    assert_eq!(state.copy_text(&message), message);
    state.cursor = *state.commands.keys().next().unwrap();
    state.toggle();
    assert_eq!(state.copy_text(&message), "git pull --rebase");
    assert_eq!(state.operation().unwrap().steps.len(), 1);
    state.toggle();
    assert!(state.marked.is_empty());
    assert!(state.operation().is_none());
    let (_, state) = MessageSelection::prepare("gpg failed to sign the data".into());
    assert!(!state.commands.is_empty());
    assert!(
        state
            .commands
            .values()
            .all(|command| !command.label.contains("false"))
    );
}

#[test]
fn selected_fixes_require_review_cancel_preserves_selection_and_execution_is_logged() {
    let (dir, mut app) = fixture();
    app.error("fatal: test failure\nhint: git config --local recovery.test 'fixed value'".into());
    assert!(app.busy.is_none());
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Char('v'));
    assert_eq!(app.message_selection.marked.len(), 1);
    key(&mut app, KeyCode::Char('v'));
    assert!(app.message_selection.marked.is_empty());
    key(&mut app, KeyCode::Char('v'));
    key(&mut app, KeyCode::Enter);
    assert!(app.busy.is_none());
    assert!(
        matches!(&app.modal, Some(Modal::Confirm { operation, .. }) if matches!(&operation.steps[0], Step::Command(command) if command.program == "git"))
    );
    key(&mut app, KeyCode::Esc);
    assert!(matches!(app.modal, Some(Modal::Message(_))));
    assert_eq!(app.message_selection.marked.len(), 1);
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Enter);
    wait(&mut app);
    assert_eq!(
        git(dir.path(), &["config", "--local", "--get", "recovery.test"]),
        "fixed value"
    );
    assert!(
        app.output
            .iter()
            .any(|line| line.contains("git config --local recovery.test"))
    );
}

#[test]
fn command_failures_open_selectable_messages_and_can_be_reopened_from_output() {
    let (_dir, mut app) = fixture();
    app.run_operation(Operation::git([
        "rev-parse",
        "--verify",
        "refs/heads/nonexistent",
    ]));
    wait(&mut app);
    assert!(matches!(app.modal, Some(Modal::Message(_))));
    assert!(app.last_command_error.is_some());
    key(&mut app, KeyCode::Esc);
    app.set_focus(Panel::Output);
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.modal, Some(Modal::Message(_))));
    key(&mut app, KeyCode::Char('v'));
    key(&mut app, KeyCode::Enter);
    assert!(app.busy.is_none());
    assert!(matches!(app.modal, Some(Modal::Message(_))));
}

#[test]
fn selected_batches_stop_on_failure_and_successful_error_text_does_not_open_a_popup() {
    let (dir, mut app) = fixture();
    app.run_operation(Operation::git([
        "-c",
        "alias.test-message=!echo error: sample text",
        "test-message",
    ]));
    wait(&mut app);
    assert!(app.modal.is_none());
    assert!(app.last_command_error.is_none());
    app.error("git rev-parse --verify refs/heads/nonexistent\ngit config --local recovery.shouldnotrun yes".into());
    key(&mut app, KeyCode::Char('v'));
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Char('v'));
    key(&mut app, KeyCode::Enter);
    assert!(
        matches!(&app.modal, Some(Modal::Confirm { operation, .. }) if operation.steps.len() == 2)
    );
    key(&mut app, KeyCode::Enter);
    wait(&mut app);
    assert!(matches!(app.modal, Some(Modal::Message(_))));
    assert!(
        !Command::new("git")
            .args(["config", "--local", "--get", "recovery.shouldnotrun"])
            .current_dir(dir.path())
            .output()
            .unwrap()
            .status
            .success()
    );
}

#[test]
fn v_toggles_local_remote_groups_and_commits_without_checkout() {
    let (dir, mut app) = fixture();
    let head = git(dir.path(), &["rev-parse", "HEAD"]);
    for (panel, name) in [
        (Panel::Branches, "feature/user/one"),
        (Panel::RemoteBranches, "origin/topic"),
    ] {
        app.set_focus(panel);
        let remote = panel == Panel::RemoteBranches;
        let row = app
            .branch_rows(remote)
            .iter()
            .position(|row| {
                row.reference
                    .is_some_and(|index| app.snapshot.references[index].name == name)
            })
            .unwrap();
        app.select_list_row(panel, row);
        let reference = app.selected_ref().unwrap().clone();
        assert!(!app.branch_visible(&reference));
        key(&mut app, KeyCode::Char('v'));
        wait(&mut app);
        assert!(app.branch_visible(&reference));
        assert!(app.busy.is_none());
        key(&mut app, KeyCode::Char('V'));
        wait(&mut app);
        assert!(!app.branch_visible(&reference));
    }
    app.set_focus(Panel::Branches);
    let group = app
        .branch_rows(false)
        .iter()
        .position(|row| row.is_group() && row.path == "feature")
        .unwrap();
    app.select_list_row(Panel::Branches, group);
    key(&mut app, KeyCode::Char('v'));
    wait(&mut app);
    assert!(
        app.snapshot
            .references
            .iter()
            .filter(|reference| reference.name.starts_with("feature/"))
            .all(|reference| app.branch_visible(reference))
    );
    key(&mut app, KeyCode::Char('v'));
    wait(&mut app);
    assert!(
        app.snapshot
            .references
            .iter()
            .filter(|reference| reference.name.starts_with("feature/"))
            .all(|reference| !app.branch_visible(reference))
    );
    assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]), head);
    app.select_graph_row(1);
    app.set_focus(Panel::Commits);
    key(&mut app, KeyCode::Char('v'));
    assert_eq!(app.copied_commits.len(), 1);
    key(&mut app, KeyCode::Char('v'));
    assert!(app.copied_commits.is_empty());
    assert!(app.busy.is_none());
}

#[test]
fn long_single_lines_and_confirmation_commands_remain_scrollable() {
    let (_dir, mut app) = fixture();
    app.error(format!("{} FINAL_TEXT", "long message ".repeat(200)));
    screen(&mut app);
    let max = app.message_max_scroll;
    assert!(max > 0);
    for _ in 0..100 {
        key(&mut app, KeyCode::PageDown);
        screen(&mut app);
    }
    assert_eq!(app.message_scroll, max);
    assert!(screen(&mut app).contains("FINAL_TEXT"));
    app.error(format!(
        "git config --local recovery.long '{}'",
        "x".repeat(1000)
    ));
    key(&mut app, KeyCode::Char('v'));
    key(&mut app, KeyCode::Enter);
    screen(&mut app);
    assert!(app.message_max_scroll > 0);
    key(&mut app, KeyCode::End);
    assert_eq!(app.message_scroll, app.message_max_scroll);
    key(&mut app, KeyCode::Esc);
    assert!(matches!(app.modal, Some(Modal::Message(_))));
}
