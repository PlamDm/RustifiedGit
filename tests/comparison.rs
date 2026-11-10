use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend};
use rustifiedgit::{
    actions,
    app::{App, Modal},
    config::{Config, State},
    git::Repository,
    model::{ComparisonRequest, Panel, PreviewTarget, Revision},
    process::Cancellation,
    ui,
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
    String::from_utf8(output.stdout).unwrap().trim().into()
}

fn wait(app: &mut App, done: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        app.poll_workers();
        if !app.loading && !app.history_loading && done(app) {
            return;
        }
        assert!(Instant::now() < deadline, "{}", app.notice);
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn fixture() -> (tempfile::TempDir, App, String) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir(&root).unwrap();
    git(&root, &["init", "-qb", "main"]);
    for (key, value) in [
        ("user.name", "Comparison Tests"),
        ("user.email", "compare@example.invalid"),
        ("commit.gpgsign", "false"),
        ("core.hooksPath", ".test-hooks"),
        ("core.autocrlf", "false"),
    ] {
        git(&root, &["config", key, value]);
    }
    fs::write(root.join("shared.txt"), "common\n").unwrap();
    fs::write(root.join("old name.txt"), "rename me\n").unwrap();
    fs::write(root.join("deleted.txt"), "delete me\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "base"]);
    let base = git(&root, &["rev-parse", "HEAD"]);
    git(&root, &["switch", "-qc", "feature"]);
    fs::write(root.join("shared.txt"), "feature\n").unwrap();
    git(&root, &["mv", "old name.txt", "new name.txt"]);
    fs::remove_file(root.join("deleted.txt")).unwrap();
    fs::write(root.join("feature-only.txt"), "feature only\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "feature changes"]);
    git(&root, &["tag", "-a", "release", "-m", "annotated release"]);
    git(
        &root,
        &["update-ref", "refs/remotes/origin/feature", "HEAD"],
    );
    git(&root, &["switch", "-q", "main"]);
    fs::write(root.join("shared.txt"), "main\n").unwrap();
    fs::write(root.join("main-only.txt"), "main only\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "main changes"]);
    fs::write(root.join("shared.txt"), "dirty worktree\n").unwrap();
    let mut config = Config::load(Some(dir.path().join("config")), &[]).unwrap();
    config.git.auto_fetch = false;
    config.git.auto_refresh = false;
    let mut app = App::new(
        Repository::discover(&root).unwrap(),
        config,
        State::default(),
    );
    wait(&mut app, |_| true);
    (dir, app, base)
}

fn request(from: &str, to: &str, merge_base: bool) -> ComparisonRequest {
    ComparisonRequest {
        from: Revision::named(from.into()),
        to: Revision::named(to.into()),
        merge_base,
    }
}

fn key(app: &mut App, code: KeyCode) {
    app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn menu_item(app: &mut App, label: &str) {
    let Some(Modal::Menu {
        items, selected, ..
    }) = &mut app.modal
    else {
        panic!("comparison menu missing")
    };
    *selected = items
        .iter()
        .position(|item| item.label.starts_with(label))
        .unwrap();
    key(app, KeyCode::Enter);
}

fn select_branch(app: &mut App, remote: bool, name: &str) {
    let row = app
        .branch_rows(remote)
        .iter()
        .position(|row| {
            row.reference
                .is_some_and(|index| app.snapshot.references[index].name == name)
        })
        .unwrap();
    app.select_list_row(
        if remote {
            Panel::RemoteBranches
        } else {
            Panel::Branches
        },
        row,
    );
}

#[test]
fn direct_and_merge_base_comparisons_handle_divergence_tags_renames_and_deletions() {
    let (_dir, app, base) = fixture();
    let token = Cancellation::default();
    let direct = app
        .repo
        .compare(&request("main", "release", false), &token)
        .unwrap();
    assert!(
        direct
            .files
            .iter()
            .any(|file| file.path == Path::new("main-only.txt") && file.index == b'D')
    );
    let rename = direct
        .files
        .iter()
        .find(|file| file.path == Path::new("new name.txt"))
        .unwrap();
    assert_eq!(rename.old_path.as_deref(), Some(Path::new("old name.txt")));
    let based = app
        .repo
        .compare(&request("main", "origin/feature", true), &token)
        .unwrap();
    assert_eq!(based.info.diff_base(), base);
    assert!(
        !based
            .files
            .iter()
            .any(|file| file.path == Path::new("main-only.txt"))
    );
    assert!(
        based
            .files
            .iter()
            .any(|file| file.path == Path::new("deleted.txt") && file.index == b'D')
    );
    let preview = |info: &rustifiedgit::model::ComparisonInfo, path, old_path| {
        app.repo
            .preview(
                &PreviewTarget::Comparison {
                    from: info.diff_base().into(),
                    to: info.to_oid.clone(),
                    path,
                    old_path,
                },
                true,
                &app.config.gui,
                3,
                false,
                &token,
            )
            .unwrap()
    };
    let patch = preview(&direct.info, "shared.txt".into(), None);
    assert!(patch.text.contains("-main\n+feature"));
    let patch = preview(&based.info, "shared.txt".into(), None);
    assert!(patch.text.contains("-common\n+feature"));
    let patch = preview(&direct.info, rename.path.clone(), rename.old_path.clone());
    assert!(patch.text.contains("rename from old name.txt"));
    assert!(patch.text.contains("rename to new name.txt"));
}

#[test]
fn branch_menu_marks_two_selections_opens_read_only_files_and_restores_graph() {
    let (_dir, mut app, _) = fixture();
    let head = git(&app.repo.root, &["rev-parse", "HEAD"]);
    let status = git(&app.repo.root, &["status", "--porcelain"]);
    select_branch(&mut app, false, "main");
    key(&mut app, KeyCode::Char('B'));
    menu_item(&mut app, "Mark main");
    select_branch(&mut app, true, "origin/feature");
    assert!(
        actions::for_app(&app)
            .iter()
            .any(|action| action.key == "B")
    );
    key(&mut app, KeyCode::Char('B'));
    menu_item(&mut app, "Compare marked");
    wait(&mut app, |app| {
        !app.comparison_loading && !app.preview_loading
    });
    assert_eq!(app.focus, Panel::CommitFiles);
    assert!(app.comparison.is_some());
    assert!(
        app.details_lines
            .iter()
            .any(|line| line.contains("main → origin/feature"))
    );
    assert!(
        app.commit_files
            .iter()
            .any(|file| file.path == Path::new("main-only.txt"))
    );
    let shared = app
        .commit_rows
        .iter()
        .position(|row| row.path == Path::new("shared.txt"))
        .unwrap();
    app.select_list_row(Panel::CommitFiles, shared);
    wait(&mut app, |app| !app.preview_loading);
    assert!(app.preview.text.contains("-main\n+feature"));
    for action in ['c', 'V', 'd'] {
        key(&mut app, KeyCode::Char(action));
        assert!(matches!(app.modal, Some(Modal::Message(_))));
        assert!(app.busy.is_none());
        key(&mut app, KeyCode::Esc);
    }
    key(&mut app, KeyCode::Enter);
    assert!(app.fullscreen);
    key(&mut app, KeyCode::Char(' '));
    assert!(app.busy.is_none());
    key(&mut app, KeyCode::Esc); // dismiss read-only message
    key(&mut app, KeyCode::Esc); // return to comparison files
    assert!(app.comparison.is_some());
    app.refresh();
    wait(&mut app, |_| true);
    assert!(app.comparison.is_some());
    assert_eq!(app.file_panel(), Panel::CommitFiles);
    key(&mut app, KeyCode::Esc);
    assert!(app.comparison.is_none());
    assert_eq!(app.focus, Panel::Commits);
    assert_eq!(git(&app.repo.root, &["rev-parse", "HEAD"]), head);
    assert_eq!(git(&app.repo.root, &["status", "--porcelain"]), status);
}

#[test]
fn typed_endpoints_base_menu_and_swapping_use_the_chosen_direction() {
    let (_dir, mut app, base) = fixture();
    select_branch(&mut app, false, "feature");
    key(&mut app, KeyCode::Char('B'));
    menu_item(&mut app, "Compare feature with base branch");
    key(&mut app, KeyCode::Enter); // suggested main
    wait(&mut app, |app| {
        !app.comparison_loading && !app.preview_loading
    });
    assert_eq!(app.comparison.as_ref().unwrap().diff_base(), base);
    assert!(
        !app.commit_files
            .iter()
            .any(|file| file.path == Path::new("main-only.txt"))
    );
    key(&mut app, KeyCode::Char('B'));
    menu_item(&mut app, "Compare two commits");
    app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    app.paste("feature".into());
    key(&mut app, KeyCode::Enter);
    app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    app.paste("main".into());
    key(&mut app, KeyCode::Enter);
    wait(&mut app, |app| {
        !app.comparison_loading && !app.preview_loading
    });
    assert!(
        app.commit_files
            .iter()
            .any(|file| file.path == Path::new("main-only.txt") && file.index == b'A')
    );
    key(&mut app, KeyCode::Char('B'));
    menu_item(&mut app, "Swap comparison direction");
    wait(&mut app, |app| {
        !app.comparison_loading && !app.preview_loading
    });
    assert!(
        app.commit_files
            .iter()
            .any(|file| file.path == Path::new("main-only.txt") && file.index == b'D')
    );
}

#[test]
fn latest_request_wins_empty_comparisons_render_and_invalid_revisions_recover() {
    let (_dir, mut app, _) = fixture();
    app.start_comparison(request("main", "feature", false));
    app.start_comparison(request("main", "main", false));
    wait(&mut app, |app| {
        !app.comparison_loading && !app.preview_loading
    });
    assert!(app.commit_files.is_empty());
    assert_eq!(app.comparison.as_ref().unwrap().summary, "No differences.");
    let mut terminal = Terminal::new(TestBackend::new(120, 36)).unwrap();
    terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
    assert!(
        app.preview_lines
            .iter()
            .any(|line| line.contains("No differences"))
    );
    app.start_comparison(request("--output=unexpected", "feature", false));
    wait(&mut app, |app| !app.comparison_loading);
    assert!(app.comparison.is_none());
    assert!(
        matches!(&app.modal, Some(Modal::Message(message)) if message.contains("Comparison failed"))
    );
    assert!(!app.repo.root.join("unexpected").exists());
    assert_eq!(git(&app.repo.root, &["branch", "--show-current"]), "main");
}

#[test]
fn graph_commit_comparison_loads_full_diffs_even_when_automatic_previews_are_disabled() {
    let (_dir, mut app, base) = fixture();
    app.state.diff_preview_enabled = false;
    app.config.gui.diff_preview_max_bytes = 1;
    app.config.gui.diff_preview_max_file_bytes = 1;
    let base_index = app
        .filtered_commits()
        .iter()
        .position(|index| app.snapshot.commits[*index].oid == base)
        .unwrap();
    app.select_graph_row(app.commit_graph_row(base_index));
    key(&mut app, KeyCode::Char('B'));
    menu_item(&mut app, "Mark ");
    let tip_index = app
        .filtered_commits()
        .iter()
        .position(|index| app.snapshot.commits[*index].oid == app.snapshot.head)
        .unwrap();
    app.select_graph_row(app.commit_graph_row(tip_index));
    key(&mut app, KeyCode::Char('B'));
    menu_item(&mut app, "Compare marked");
    wait(&mut app, |app| {
        !app.comparison_loading && !app.preview_loading
    });
    let shared = app
        .commit_rows
        .iter()
        .position(|row| row.path == Path::new("shared.txt"))
        .unwrap();
    app.select_list_row(Panel::CommitFiles, shared);
    wait(&mut app, |app| !app.preview_loading);
    assert!(app.preview.text.contains("-common\n+main"));
    assert!(!app.preview.suppressed);
    assert_eq!(app.comparison.as_ref().unwrap().from_oid, base);
    key(&mut app, KeyCode::Char('5'));
    assert!(app.comparison.is_none());
}

#[test]
fn missing_common_ancestor_is_explained_and_direct_comparison_still_works() {
    let (_dir, app, _) = fixture();
    let tree = git(&app.repo.root, &["rev-parse", "HEAD^{tree}"]);
    let unrelated = git(
        &app.repo.root,
        &["commit-tree", &tree, "-m", "independent root"],
    );
    let token = Cancellation::default();
    let error = app
        .repo
        .compare(&request("main", &unrelated, true), &token)
        .unwrap_err()
        .to_string();
    assert!(error.contains("no common ancestor"), "{error}");
    let direct = app
        .repo
        .compare(&request("main", &unrelated, false), &token)
        .unwrap();
    assert!(direct.files.is_empty());
}
