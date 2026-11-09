use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use pd_lazygit::{
    actions,
    app::{App, MenuAction, Modal},
    config::{Config, State},
    git::{Operation, Repository, Step},
    model::Panel,
};
use std::{
    fs,
    path::Path,
    process::Command,
    time::{Duration, Instant},
};

fn git(root: &Path, args: &[&str]) -> String {
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
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn fixture() -> (tempfile::TempDir, App) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir(&root).unwrap();
    git(&root, &["init", "-qb", "main"]);
    for (name, value) in [
        ("user.name", "Remote UI Test"),
        ("user.email", "remote@example.invalid"),
        ("commit.gpgsign", "false"),
        ("core.hooksPath", ".test-hooks"),
    ] {
        git(&root, &["config", name, value]);
    }
    fs::write(root.join("file.txt"), "original\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "initial"]);
    git(&root, &["branch", "topic"]);
    let remote = dir.path().join("nonexistent-local-remote.git");
    git(
        &root,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&root, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    git(&root, &["branch", "--set-upstream-to=origin/main", "main"]);
    fs::write(root.join("file.txt"), "working changes\n").unwrap();
    let mut config = Config::load(Some(dir.path().join("config")), &[]).unwrap();
    config.git.auto_refresh = false;
    config.git.auto_fetch = false;
    let mut app = App::new(
        Repository::discover(&root).unwrap(),
        config,
        State::default(),
    );
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        app.poll_workers();
        if !app.loading && !app.history_loading {
            break;
        }
        assert!(Instant::now() < deadline, "timed out: {}", app.notice);
        std::thread::sleep(Duration::from_millis(5));
    }
    (dir, app)
}

fn key(app: &mut App, code: KeyCode) {
    app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn commands(operation: &Operation) -> Vec<Vec<String>> {
    operation
        .steps
        .iter()
        .filter_map(|step| match step {
            Step::Command(spec) => {
                assert_eq!(spec.program, "git");
                Some(
                    spec.args
                        .iter()
                        .map(|argument| argument.to_string_lossy().into_owned())
                        .collect(),
                )
            }
            _ => None,
        })
        .collect()
}

fn menu_operations(app: &App) -> Vec<(&str, &Operation, bool)> {
    match app.modal.as_ref() {
        Some(Modal::Menu { items, .. }) => items
            .iter()
            .filter_map(|item| match &item.action {
                MenuAction::Run(operation, confirm) => {
                    Some((item.label.as_str(), operation, *confirm))
                }
                _ => None,
            })
            .collect(),
        _ => panic!("expected an options menu: {}", app.notice),
    }
}

#[test]
fn pull_and_push_keys_open_options_from_primary_panes_without_executing_commands() {
    let (_dir, mut app) = fixture();
    let head = git(&app.repo.root, &["rev-parse", "HEAD"]);
    for panel in [
        Panel::Commits,
        Panel::Files,
        Panel::Status,
        Panel::Branches,
        Panel::RemoteBranches,
    ] {
        app.set_focus(panel);
        for character in ['p', 'P'] {
            key(&mut app, KeyCode::Char(character));
            assert!(
                matches!(app.modal, Some(Modal::Menu { .. })),
                "{panel:?} {character}: {}",
                app.notice
            );
            assert!(app.busy.is_none(), "opening options ran a Git command");
            assert!(app.output.is_empty(), "opening options wrote a command log");
            key(&mut app, KeyCode::Esc);
        }
    }
    assert_eq!(git(&app.repo.root, &["rev-parse", "HEAD"]), head);
    assert_eq!(
        fs::read_to_string(app.repo.root.join("file.txt")).unwrap(),
        "working changes\n"
    );
}

#[test]
fn pull_menu_generates_ff_merge_rebase_and_autostash_native_commands() {
    let (_dir, mut app) = fixture();
    key(&mut app, KeyCode::Char('p'));
    let operations = menu_operations(&app);
    for required in [
        vec!["--ff-only"],
        vec!["--no-rebase"],
        vec!["--rebase"],
        vec!["--rebase", "--autostash"],
    ] {
        assert!(
            operations
                .iter()
                .any(|(_, operation, _)| commands(operation).iter().any(|args| {
                    args.first().is_some_and(|first| first == "pull")
                        && required
                            .iter()
                            .all(|flag| args.iter().any(|argument| argument == flag))
                })),
            "missing pull options {required:?}"
        );
    }
    let destructive = operations
        .iter()
        .find(|(label, _, _)| {
            let label = label.to_lowercase();
            label.contains("overwrite") || label.contains("discard")
        })
        .expect("pull overwrite option missing");
    assert!(destructive.2, "overwrite pull did not require confirmation");
    assert!(app.busy.is_none());
}

#[test]
fn force_push_options_require_confirmation_and_cancellation_does_not_execute() {
    let (_dir, mut app) = fixture();
    for force in ["--force-with-lease", "--force"] {
        key(&mut app, KeyCode::Char('P'));
        let target = match app.modal.as_ref() {
            Some(Modal::Menu { items, .. }) => items
                .iter()
                .position(|item| match &item.action {
                    MenuAction::Run(operation, confirm) => {
                        commands(operation).iter().any(|args| {
                            args.first().is_some_and(|arg| arg == "push")
                                && args.iter().any(|arg| arg == force)
                        }) && *confirm
                    }
                    _ => false,
                })
                .expect("confirmed force push option missing"),
            _ => panic!("push options missing"),
        };
        for _ in 0..target {
            key(&mut app, KeyCode::Down);
        }
        key(&mut app, KeyCode::Enter);
        assert!(matches!(app.modal, Some(Modal::Confirm { .. })));
        assert!(app.busy.is_none());
        key(&mut app, KeyCode::Esc);
        assert!(app.busy.is_none());
        assert!(app.output.is_empty());
    }
}

#[test]
fn push_menu_includes_normal_dry_run_and_upstream_choices_and_actions_advertise_menus() {
    let (_dir, mut app) = fixture();
    key(&mut app, KeyCode::Char('P'));
    let operations = menu_operations(&app);
    assert!(
        operations.iter().any(
            |(_, operation, _)| commands(operation).iter().any(|args| args
                .first()
                .is_some_and(|arg| arg == "push")
                && !args
                    .iter()
                    .any(|arg| arg.starts_with("--force") || arg == "--dry-run"))
        )
    );
    assert!(operations.iter().any(|(_, operation, _)| {
        commands(operation)
            .iter()
            .any(|args| args.iter().any(|arg| arg == "--dry-run"))
    }));
    let upstream_choice = match app.modal.as_ref() {
        Some(Modal::Menu { items, .. }) => items
            .iter()
            .any(|item| item.label.to_lowercase().contains("upstream")),
        _ => false,
    };
    assert!(upstream_choice);
    key(&mut app, KeyCode::Esc);
    let catalog = actions::for_app(&app);
    for character in ['p', 'P'] {
        let action = catalog
            .iter()
            .find(|action| action.event.code == KeyCode::Char(character))
            .expect("remote menu absent from context actions");
        assert!(
            action.label.to_lowercase().contains("option")
                || action.description.to_lowercase().contains("menu"),
            "{}",
            action.description
        );
    }
}

#[test]
fn pushing_another_local_branch_targets_that_reference_without_checking_it_out() {
    let (_dir, mut app) = fixture();
    app.set_focus(Panel::Branches);
    let topic = app
        .branch_rows(false)
        .iter()
        .position(|row| {
            row.reference
                .is_some_and(|index| app.snapshot.references[index].name == "topic")
        })
        .unwrap();
    app.select_list_row(Panel::Branches, topic);
    key(&mut app, KeyCode::Char('P'));
    let operations = menu_operations(&app);
    assert!(!operations.is_empty());
    for (_, operation, _) in operations {
        for args in commands(operation) {
            assert_eq!(args.first().map(String::as_str), Some("push"));
            assert!(args.iter().any(|arg| arg == "origin"), "{args:?}");
            assert!(args.iter().any(|arg| arg == "refs/heads/topic"), "{args:?}");
            assert!(
                !args
                    .iter()
                    .any(|arg| arg == "refs/heads/main" || arg == "HEAD"),
                "{args:?}"
            );
        }
    }
    assert!(app.busy.is_none());
    assert_eq!(
        git(&app.repo.root, &["symbolic-ref", "--short", "HEAD"]),
        "main"
    );
    key(&mut app, KeyCode::Esc);
}

#[test]
fn remote_branch_push_menu_uses_current_branch_instead_of_publishing_tracking_refs() {
    let (_dir, mut app) = fixture();
    app.set_focus(Panel::RemoteBranches);
    let remote = app
        .branch_rows(true)
        .iter()
        .position(|row| row.reference.is_some())
        .unwrap();
    app.select_list_row(Panel::RemoteBranches, remote);
    key(&mut app, KeyCode::Char('P'));
    for (_, operation, _) in menu_operations(&app) {
        for args in commands(operation) {
            assert!(
                !args.iter().any(|arg| arg.starts_with("refs/remotes/")),
                "{args:?}"
            );
        }
    }
    assert!(app.busy.is_none());
}

#[test]
fn remote_menus_honor_remapped_shortcuts_in_branch_contexts() {
    let (_dir, mut app) = fixture();
    let universal = app.config.keybinding.entry("universal".into()).or_default();
    universal.insert("pull".into(), serde_yaml::Value::String("x".into()));
    universal.insert("push".into(), serde_yaml::Value::String("z".into()));
    app.set_focus(Panel::Branches);
    for (original, replacement) in [('p', 'x'), ('P', 'z')] {
        key(&mut app, KeyCode::Char(original));
        assert!(
            app.modal.is_none(),
            "old key {original} still opened a menu"
        );
        assert!(app.busy.is_none(), "old key {original} still ran a command");
        key(&mut app, KeyCode::Char(replacement));
        assert!(matches!(app.modal, Some(Modal::Menu { .. })));
        key(&mut app, KeyCode::Esc);
        assert!(
            actions::for_app(&app)
                .iter()
                .any(|action| action.event.code == KeyCode::Char(replacement))
        );
    }
}
