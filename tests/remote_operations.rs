use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use rustifiedgit::{
    app::{App, Modal},
    config::{Config, State},
    git::Repository,
};
use std::{
    fs,
    path::{Path, PathBuf},
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
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}
fn configure(root: &Path) {
    for (name, value) in [
        ("user.name", "Remote Tests"),
        ("user.email", "remote@example.invalid"),
        ("commit.gpgsign", "false"),
        ("core.hooksPath", ".test-hooks"),
    ] {
        git(root, &["config", name, value]);
    }
}
fn wait(app: &mut App) {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        app.poll_workers();
        if app.busy.is_none() && !app.loading && !app.history_loading {
            return;
        }
        assert!(Instant::now() < deadline, "timed out: {}", app.notice);
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn key(app: &mut App, code: KeyCode) {
    app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
}
fn choose(app: &mut App, shortcut: char, label: &str, confirm: bool) {
    key(app, KeyCode::Char(shortcut));
    let selected = match app.modal.as_ref() {
        Some(Modal::Menu { items, .. }) => items
            .iter()
            .position(|item| item.label.starts_with(label))
            .unwrap(),
        _ => panic!("options missing"),
    };
    for _ in 0..selected {
        key(app, KeyCode::Down);
    }
    key(app, KeyCode::Enter);
    if confirm {
        assert!(matches!(app.modal, Some(Modal::Confirm { .. })));
        assert!(app.busy.is_none());
        key(app, KeyCode::Enter);
    }
    wait(app);
}
fn fixture() -> (tempfile::TempDir, App, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let local = dir.path().join("local");
    let remote = dir.path().join("origin.git");
    let other = dir.path().join("other");
    fs::create_dir(&local).unwrap();
    git(&local, &["init", "-qb", "main"]);
    configure(&local);
    for name in ["local.txt", "remote.txt"] {
        fs::write(local.join(name), "base\n").unwrap();
    }
    git(&local, &["add", "."]);
    git(&local, &["commit", "-qm", "base"]);
    git(
        dir.path(),
        &[
            "clone",
            "--bare",
            local.to_str().unwrap(),
            remote.to_str().unwrap(),
        ],
    );
    git(
        &local,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&local, &["fetch", "-q", "origin"]);
    git(&local, &["branch", "--set-upstream-to=origin/main"]);
    git(
        dir.path(),
        &["clone", remote.to_str().unwrap(), other.to_str().unwrap()],
    );
    configure(&other);
    let mut config = Config::load(Some(dir.path().join("config")), &[]).unwrap();
    config.git.auto_fetch = false;
    config.git.auto_refresh = false;
    let mut app = App::new(
        Repository::discover(&local).unwrap(),
        config,
        State::default(),
    );
    wait(&mut app);
    (dir, app, remote, other)
}

#[test]
fn selected_rebase_autostash_mode_rebases_and_restores_dirty_changes() {
    let (_dir, mut app, _remote, other) = fixture();
    fs::write(other.join("remote.txt"), "remote commit\n").unwrap();
    git(&other, &["commit", "-qam", "remote update"]);
    git(&other, &["push", "-q"]);
    fs::write(app.repo.root.join("local-commit.txt"), "local commit\n").unwrap();
    git(&app.repo.root, &["add", "."]);
    git(&app.repo.root, &["commit", "-qm", "local update"]);
    fs::write(app.repo.root.join("local.txt"), "dirty local contents\n").unwrap();
    git(&app.repo.root, &["config", "pull.ff", "only"]);
    choose(&mut app, 'p', "Rebase + autostash", false);
    assert_eq!(
        fs::read_to_string(app.repo.root.join("local.txt")).unwrap(),
        "dirty local contents\n"
    );
    assert_eq!(
        fs::read_to_string(app.repo.root.join("remote.txt")).unwrap(),
        "remote commit\n"
    );
    assert!(app.repo.root.join("local-commit.txt").exists());
    assert_eq!(
        git(&app.repo.root, &["log", "-1", "--format=%s"]),
        "local update"
    );
    git(
        &app.repo.root,
        &["merge-base", "--is-ancestor", "origin/main", "HEAD"],
    );
    assert!(
        app.output
            .iter()
            .any(|line| line.contains("Command completed"))
    );
}

#[test]
fn lease_rejects_unseen_remote_commit_and_explicit_force_can_replace_it() {
    let (_dir, mut app, remote, other) = fixture();
    let original = git(&app.repo.root, &["rev-parse", "HEAD"]);
    fs::write(other.join("remote.txt"), "unseen remote commit\n").unwrap();
    git(&other, &["commit", "-qam", "unseen update"]);
    git(&other, &["push", "-q"]);
    let remote_head = git(&remote, &["rev-parse", "main"]);
    choose(&mut app, 'P', "Force push with lease", true);
    assert_eq!(git(&remote, &["rev-parse", "main"]), remote_head);
    assert!(
        app.output.iter().any(|line| line.contains("stale info")),
        "{:?}",
        app.output
    );
    choose(&mut app, 'P', "Force push (", true);
    assert_eq!(git(&remote, &["rev-parse", "main"]), original);
    assert!(
        app.output
            .iter()
            .any(|line| line.contains("Command completed"))
    );
}
