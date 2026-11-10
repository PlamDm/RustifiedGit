#![cfg(unix)]

use ratatui::{Terminal, backend::TestBackend};
use rustifiedgit::{
    app::App,
    config::{Config, State},
    git::{Operation, Repository, Step},
    ui,
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
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn wait(app: &mut App, done: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        app.poll_workers();
        if done(app) {
            return;
        }
        assert!(Instant::now() < deadline, "{}", app.notice);
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn fixture() -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("repo");
    fs::create_dir(&root).unwrap();
    git(&root, &["init", "-qb", "main"]);
    for (name, value) in [
        ("user.name", "Progress Tests"),
        ("user.email", "progress@example.invalid"),
        ("commit.gpgsign", "false"),
        ("core.hooksPath", ".test-hooks"),
    ] {
        git(&root, &["config", name, value]);
    }
    git(&root, &["commit", "--allow-empty", "-qm", "base"]);
    let mut config = Config::load(Some(directory.path().join("config")), &[]).unwrap();
    config.git.auto_fetch = false;
    config.git.auto_refresh = false;
    let mut app = App::new(
        Repository::discover(&root).unwrap(),
        config,
        State::default(),
    );
    wait(&mut app, |app| !app.loading && !app.history_loading);
    Fixture {
        _directory: directory,
        app,
    }
}

#[test]
fn progress_replaces_the_live_row_before_completion_and_preserves_split_crlf() {
    let mut fixture = fixture();
    let app = &mut fixture.app;
    let mut operation = Operation::shell(
        "printf 'Updating files: 10%% (1/10)\\r' >&2; \
         while [ ! -f .progress-two ]; do sleep 0.01; done; \
         printf '\\033[2KUpdating files: 80%% (8/10)\\r' >&2; \
         while [ ! -f .progress-three ]; do sleep 0.01; done; \
         printf '\\033[2KUpdating files: 100%% (10/10), done.\\r' >&2; \
         while [ ! -f .progress-finish ]; do sleep 0.01; done; \
         printf '\\n' >&2"
            .into(),
    );
    operation.label = "git switch (progress fixture)".into();
    app.run_operation(operation);
    wait(app, |app| app.output_tail().contains("10%"));
    assert!(app.busy.is_some());
    fs::write(app.repo.root.join(".progress-two"), "").unwrap();
    wait(app, |app| app.output_tail().contains("80%"));
    assert!(!app.output_tail().contains("10%"));
    assert!(app.busy.is_some());

    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal.draw(|frame| ui::draw(frame, app)).unwrap();
    let buffer = terminal.backend().buffer();
    let screen = (0..30)
        .map(|y| {
            (0..100)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(screen.contains("80% (8/10)"), "{screen}");
    assert!(screen.contains("switching"), "{screen}");
    assert!(!app.output_tail().contains('\r'));

    fs::write(app.repo.root.join(".progress-three"), "").unwrap();
    wait(app, |app| app.output_tail().contains("100%"));
    fs::write(app.repo.root.join(".progress-finish"), "").unwrap();
    wait(app, |app| app.busy.is_none());
    assert!(
        app.output
            .iter()
            .any(|line| line == "Updating files: 100% (10/10), done.")
    );
    assert!(app.output.iter().all(|line| !line.contains('\r')));
    assert!(app.output_tail().is_empty());
}

#[test]
fn captured_switch_reports_real_git_progress_and_enables_filter_transfer_progress() {
    let mut fixture = fixture();
    let app = &mut fixture.app;
    git(&app.repo.root, &["switch", "-qc", "feature"]);
    fs::write(
        app.repo.root.join(".gitattributes"),
        "*.bin filter=progress\n",
    )
    .unwrap();
    git(
        &app.repo.root,
        &[
            "config",
            "filter.progress.smudge",
            "printf 'filter transfer progress: %s\\n' \"$GIT_LFS_FORCE_PROGRESS\" >&2; cat",
        ],
    );
    for index in 0..100 {
        fs::write(
            app.repo.root.join(format!("file-{index}.bin")),
            "contents\n",
        )
        .unwrap();
    }
    git(&app.repo.root, &["add", "."]);
    git(&app.repo.root, &["commit", "-qm", "many files"]);
    git(&app.repo.root, &["switch", "-q", "main"]);
    let mut operation = Operation::git(["switch", "--", "feature"]);
    let Step::Command(command) = &mut operation.steps[0] else {
        panic!("missing switch command")
    };
    // Avoid Git's delayed progress display so this fixture stays fast.
    command.env.push(("GIT_PROGRESS_DELAY".into(), "0".into()));
    app.run_operation(operation);
    wait(app, |app| app.busy.is_none());
    assert_eq!(
        git(&app.repo.root, &["branch", "--show-current"]),
        "feature"
    );
    assert!(
        app.output
            .iter()
            .any(|line| line.contains("Updating files: 100%"))
    );
    assert!(
        app.output
            .iter()
            .any(|line| line == "filter transfer progress: 1")
    );
    assert!(
        app.output
            .iter()
            .any(|line| line.starts_with("> git switch --progress"))
    );
}
