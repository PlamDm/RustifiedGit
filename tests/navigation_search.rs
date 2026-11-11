use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend, style::Modifier};
use rustifiedgit::{
    app::App,
    config::{Config, State},
    fuzzy::{self, Query},
    git::Repository,
    model::Panel,
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
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn commit(root: &Path, subject: &str, date: &str) -> String {
    git(root, &["add", "."]);
    let output = Command::new("git")
        .args(["commit", "-qm", subject])
        .current_dir(root)
        .env("GIT_AUTHOR_DATE", date)
        .env("GIT_COMMITTER_DATE", date)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    git(root, &["rev-parse", "HEAD"])
}

fn wait(app: &mut App, done: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        app.poll_workers();
        if !app.loading && !app.history_loading && done(app) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out: {}", app.notice);
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn key(app: &mut App, code: KeyCode) {
    app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
}
fn type_query(app: &mut App, query: &str) {
    key(app, KeyCode::Char('/'));
    app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    for character in query.chars() {
        key(app, KeyCode::Char(character));
    }
}

fn fixture() -> (tempfile::TempDir, App, String, String) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir(&root).unwrap();
    git(&root, &["init", "-qb", "main"]);
    for (name, value) in [
        ("user.name", "Search Tester"),
        ("user.email", "test@example.invalid"),
        ("commit.gpgsign", "false"),
        ("core.hooksPath", ".test-hooks"),
    ] {
        git(&root, &["config", name, value]);
    }
    fs::create_dir(root.join("src")).unwrap();
    fs::write(root.join("src/shared.txt"), "base contents\n").unwrap();
    fs::write(root.join("src/deleted.txt"), "HEAD-only deleted contents\n").unwrap();
    let base = commit(&root, "base common", "2026-01-01T12:00:00Z");
    fs::write(root.join("src/shared.txt"), "HEAD committed contents\n").unwrap();
    let head = commit(&root, "main needle", "2026-01-02T12:00:00Z");
    git(&root, &["switch", "-qc", "feature", &base]);
    fs::write(root.join("feature.rs"), "feature contents\n").unwrap();
    let feature = commit(&root, "feature needle", "2026-02-01T12:00:00Z");
    git(&root, &["config", "remote.origin.url", "."]);
    git(
        &root,
        &[
            "config",
            "remote.origin.fetch",
            "+refs/heads/*:refs/remotes/origin/*",
        ],
    );
    git(&root, &["update-ref", "refs/remotes/origin/main", &feature]);
    git(&root, &["switch", "-q", "main"]);
    git(&root, &["branch", "--set-upstream-to=origin/main", "main"]);
    fs::write(root.join("src/shared.txt"), "dirty contents\n").unwrap();
    fs::remove_file(root.join("src/deleted.txt")).unwrap();
    fs::write(root.join("untracked.txt"), "not in HEAD\n").unwrap();
    let mut config = Config::load(Some(dir.path().join("config")), &[]).unwrap();
    config.git.auto_fetch = false;
    let mut app = App::new(
        Repository::discover(&root).unwrap(),
        config,
        State::default(),
    );
    wait(&mut app, |_| true);
    app.options.visible_refs = Some(vec!["refs/heads/main".into(), "refs/heads/feature".into()]);
    app.refresh();
    wait(&mut app, |_| true);
    (dir, app, head, feature)
}

fn screen(app: &mut App) -> Terminal<TestBackend> {
    let mut terminal = Terminal::new(TestBackend::new(180, 48)).unwrap();
    terminal.draw(|frame| ui::draw(frame, app)).unwrap();
    terminal
}
fn text(terminal: &Terminal<TestBackend>) -> String {
    let buffer = terminal.backend().buffer();
    (0..48)
        .map(|y| {
            (0..180)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn working_tree_follows_head_and_navigation_preserves_real_commits() {
    let (_dir, mut app, head, feature) = fixture();
    assert_eq!(app.snapshot.commits[0].oid, feature);
    let insertion = app.working_tree_graph_index();
    assert!(insertion > 0);
    app.select_graph_row(insertion);
    assert!(app.graph_working_tree);
    key(&mut app, KeyCode::Down);
    assert_eq!(app.selected_commit().unwrap().oid, head);
    key(&mut app, KeyCode::Up);
    assert!(app.graph_working_tree);
    key(&mut app, KeyCode::Up);
    assert_eq!(app.selected_commit().unwrap().oid, feature);
    let rendered = text(&screen(&mut app));
    let feature_row = rendered
        .lines()
        .position(|line| line.contains("feature needle"))
        .unwrap();
    let worktree_row = rendered
        .lines()
        .position(|line| line.contains("Working tree / Index"))
        .unwrap();
    let head_row = rendered
        .lines()
        .position(|line| line.contains("main needle"))
        .unwrap();
    assert!(feature_row < worktree_row);
    assert_eq!(worktree_row + 1, head_row);
    let area = ui::areas(ratatui::layout::Rect::new(0, 0, 180, 48), &app).graph;
    assert_eq!(
        ui::hit_test(
            ratatui::layout::Rect::new(0, 0, 180, 48),
            &app,
            area.x + 3,
            worktree_row as u16
        ),
        Some((Panel::Commits, insertion))
    );
    key(&mut app, KeyCode::Home);
    assert_eq!(app.selected_commit().unwrap().oid, feature);
    key(&mut app, KeyCode::End);
    assert_eq!(app.selected_commit().unwrap().subject, "base common");
    assert_eq!(git(&app.repo.root, &["rev-parse", "HEAD"]), head);
}

#[test]
fn active_branch_is_bold_and_tracking_counts_are_visible() {
    let (_dir, mut app, _, _) = fixture();
    let terminal = screen(&mut app);
    let rendered = text(&terminal);
    assert!(rendered.contains("AHEAD 1 · BEHIND 1"));
    let buffer = terminal.backend().buffer();
    let local = ui::areas(ratatui::layout::Rect::new(0, 0, 180, 48), &app).local;
    let mut found = false;
    for y in local.y + 1..local.bottom() - 1 {
        for x in local.x + 1..local.right().saturating_sub(4) {
            let word: String = (x..x + 4).map(|x| buffer[(x, y)].symbol()).collect();
            if word == "main" {
                found = true;
                assert!(buffer[(x, y)].modifier.contains(Modifier::BOLD));
            }
        }
    }
    assert!(found);
}

#[test]
fn live_commit_search_accepts_cancels_ranks_and_drives_changed_files() {
    let (_dir, mut app, head, feature) = fixture();
    let original = app.selected_graph_row();
    type_query(&mut app, "feature needle");
    assert!(app.modal.is_none());
    assert!(app.search_edit.is_some());
    assert_eq!(app.filtered_commits().len(), 1);
    assert_eq!(app.selected_commit().unwrap().oid, feature);
    wait(&mut app, |app| !app.inspection_loading);
    assert!(
        app.commit_files
            .iter()
            .any(|file| file.path == Path::new("feature.rs"))
    );
    assert!(text(&screen(&mut app)).contains("1 matches (loaded)"));
    key(&mut app, KeyCode::Esc);
    assert!(app.search(Panel::Commits).is_empty());
    assert_eq!(app.selected_graph_row(), original);
    type_query(&mut app, "main needle");
    key(&mut app, KeyCode::Enter);
    assert!(app.search_edit.is_none());
    assert_eq!(app.selected_commit().unwrap().oid, head);
    key(&mut app, KeyCode::Esc);
    assert!(app.search(Panel::Commits).is_empty());
    assert_eq!(app.selected_commit().unwrap().oid, head);
    type_query(&mut app, "missing-no-result");
    assert!(app.filtered_commits().is_empty());
    let _ = screen(&mut app);
    key(&mut app, KeyCode::Esc);
}

#[test]
fn live_branch_search_uses_ctrl_f_and_restores_the_selected_branch_on_cancel() {
    let (_dir, mut app, _, _) = fixture();
    let main_row = app
        .branch_rows(false)
        .iter()
        .position(|row| {
            row.reference
                .is_some_and(|index| app.snapshot.references[index].name == "main")
        })
        .unwrap();
    app.select_list_row(Panel::Branches, main_row);
    app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL));
    assert!(app.search_edit.is_some());
    for character in "feat".chars() {
        key(&mut app, KeyCode::Char(character));
    }
    assert_eq!(app.refs_for(false).len(), 1);
    assert!(
        app.branch_rows(false)
            .iter()
            .any(|row| row.path == "feature")
    );
    assert!(
        app.search_hint(Panel::Branches)
            .unwrap()
            .contains("1 matches")
    );
    key(&mut app, KeyCode::Esc);
    assert!(app.search(Panel::Branches).is_empty());
    assert_eq!(app.selected_branch_row().unwrap().path, "main");

    app.set_focus(Panel::RemoteBranches);
    app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL));
    for character in "origin/main".chars() {
        key(&mut app, KeyCode::Char(character));
    }
    assert_eq!(app.refs_for(true).len(), 1);
    assert!(
        app.search_hint(Panel::RemoteBranches)
            .unwrap()
            .contains("1 matches")
    );
    key(&mut app, KeyCode::Enter);
    assert!(app.search_edit.is_none());
    assert_eq!(app.search(Panel::RemoteBranches), "origin/main");
}

#[test]
fn file_tabs_keep_head_contents_and_filters_independent_of_hovered_commit() {
    let (_dir, mut app, head, feature) = fixture();
    let feature_index = app
        .filtered_commits()
        .iter()
        .position(|index| app.snapshot.commits[*index].oid == feature)
        .unwrap();
    app.select_graph_row(app.commit_graph_row(feature_index));
    wait(&mut app, |app| !app.inspection_loading);
    key(&mut app, KeyCode::Char('7'));
    assert_eq!(app.focus, Panel::CommitFiles);
    type_query(&mut app, "feature");
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Char(']'));
    wait(&mut app, |app| !app.head_loading);
    assert_eq!(app.focus, Panel::HeadFiles);
    assert_eq!(app.head_oid, head);
    assert!(
        app.head_files
            .iter()
            .any(|file| file.path == Path::new("src/deleted.txt"))
    );
    assert!(!app.head_files.iter().any(
        |file| file.path == Path::new("feature.rs") || file.path == Path::new("untracked.txt")
    ));
    type_query(&mut app, "shared");
    key(&mut app, KeyCode::Enter);
    let index = app
        .head_rows
        .iter()
        .position(|row| row.path == Path::new("src/shared.txt"))
        .unwrap();
    app.select_list_row(Panel::HeadFiles, index);
    key(&mut app, KeyCode::Enter);
    wait(&mut app, |app| !app.preview_loading);
    assert_eq!(app.focus, Panel::Diff);
    assert!(app.fullscreen);
    assert_eq!(app.preview.text, "HEAD committed contents\n");
    assert!(app.patch.hunks.is_empty());
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Char('['));
    assert_eq!(app.focus, Panel::CommitFiles);
    assert_eq!(app.search(Panel::CommitFiles), "feature");
    assert_eq!(app.search(Panel::HeadFiles), "shared");
    key(&mut app, KeyCode::Char(']'));
    app.select_graph_row(app.working_tree_graph_index());
    assert_eq!(app.file_panel(), Panel::HeadFiles);
    assert_eq!(app.side, Panel::HeadFiles);
    key(&mut app, KeyCode::Char('7'));
    key(&mut app, KeyCode::Char('['));
    assert_eq!(app.focus, Panel::Files);
    assert!(
        app.rows
            .iter()
            .any(|row| row.path == Path::new("untracked.txt"))
    );
    assert_eq!(git(&app.repo.root, &["rev-parse", "HEAD"]), head);
}

#[test]
fn incremental_and_parallel_commit_search_preserve_full_scan_ranking_and_query_operators() {
    use rustifiedgit::model::Commit;
    use std::sync::Arc;
    let (_dir, mut app, _, _) = fixture();
    let subjects = [
        "Parser fix",
        "parser tests",
        "path refactor",
        "Parse docs",
        "fix İSTANBUL ΟΣ",
        "bug parser",
        "reparse",
    ];
    Arc::make_mut(&mut app.snapshot).commits = Arc::new(
        (0..17_000)
            .map(|index| Commit {
                oid: format!("{index:040x}"),
                subject: subjects[index % subjects.len()].into(),
                author: "Example Author".into(),
                ..Commit::default()
            })
            .collect(),
    );
    for text in [
        "p",
        "pa",
        "par",
        "parser",
        "parse",
        "parser !bug",
        "parser !bugs",
        "'parser",
        "^parser",
        "Pa",
        "par",
        "İ",
        "ος",
        "",
    ] {
        app.searches[3] = text.into();
        let query = Query::new(text);
        let mut expected: Vec<_> = app
            .snapshot
            .commits
            .iter()
            .enumerate()
            .filter_map(|(index, commit)| {
                query
                    .score(&format!(
                        "{} {} {} {} {}",
                        commit.oid,
                        commit.subject,
                        commit.author,
                        commit.committer,
                        commit.decorations
                    ))
                    .map(|score| {
                        (
                            index,
                            score
                                + query
                                    .score(&commit.subject)
                                    .map_or(0, |subject| 128 + subject.max(0)),
                        )
                    })
            })
            .collect();
        expected.sort_unstable_by(|left, right| {
            right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0))
        });
        let expected: Vec<_> = expected.into_iter().map(|(index, _)| index).collect();
        assert_eq!(
            &*app.filtered_commits(),
            expected.as_slice(),
            "query: {text}"
        );
    }
}

#[test]
fn fuzzy_queries_rank_boundaries_and_support_terms_exclusions_and_unicode() {
    assert!(
        fuzzy::score("src/cfg.rs", "cfg").unwrap()
            > fuzzy::score("src/coffee_long_gap.rs", "cfg").unwrap()
    );
    assert!(Query::new("src rs !test").is_match("src/config.rs"));
    assert!(!Query::new("src rs !test").is_match("src/config_test.rs"));
    assert!(Query::new("'config").is_match("src/config.rs"));
    assert!(!Query::new("'cfg").is_match("src/config.rs"));
    assert!(Query::new("^src .rs$").is_match("src/config.rs"));
    assert!(Query::new("пр").is_match("Промяна"));
    assert!(!Query::new("Foo").is_match("foo"));
    assert_eq!(Query::new("  ").score("anything"), Some(0));
}

#[test]
fn head_tree_refreshes_after_head_moves_and_keeps_its_active_tab() {
    let (_dir, mut app, _, _) = fixture();
    app.set_focus(Panel::HeadFiles);
    wait(&mut app, |app| !app.head_loading);
    fs::write(
        app.repo.root.join("new-head.txt"),
        "new committed contents\n",
    )
    .unwrap();
    let new_head = commit(&app.repo.root, "updated head", "2026-03-01T12:00:00Z");
    app.refresh();
    wait(&mut app, |app| {
        !app.head_loading && app.head_oid == new_head
    });
    assert_eq!(app.file_panel(), Panel::HeadFiles);
    assert!(
        app.head_files
            .iter()
            .any(|file| file.path == Path::new("new-head.txt"))
    );
    assert!(
        !app.head_files
            .iter()
            .any(|file| file.path == Path::new("src/deleted.txt"))
    );
    type_query(&mut app, "newhead");
    assert_eq!(app.head_rows.len(), 1);
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Enter);
    wait(&mut app, |app| !app.preview_loading);
    assert_eq!(app.preview.text, "new committed contents\n");
}

#[test]
fn hidden_head_has_worktree_fallback_and_narrow_header_keeps_tracking_counts() {
    let (_dir, mut app, head, _) = fixture();
    app.options.visible_refs = Some(vec!["refs/heads/feature".into()]);
    app.refresh();
    wait(&mut app, |_| true);
    assert!(!app.snapshot.commits.iter().any(|commit| commit.oid == head));
    assert_eq!(app.working_tree_graph_index(), 0);
    app.select_graph_row(0);
    assert!(app.graph_working_tree);
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
    let buffer = terminal.backend().buffer();
    let header: String = (0..80).map(|x| buffer[(x, 0)].symbol()).collect();
    assert!(header.contains("AHEAD 1 · BEHIND 1"), "{header}");
}

#[test]
fn unborn_head_tree_is_empty_and_search_is_safe() {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-qb", "main"]);
    let config = Config::load(Some(dir.path().join("config")), &[]).unwrap();
    let mut app = App::new(
        Repository::discover(dir.path()).unwrap(),
        config,
        State::default(),
    );
    wait(&mut app, |_| true);
    app.set_focus(Panel::HeadFiles);
    assert!(app.head_files.is_empty());
    assert!(!app.head_loading);
    type_query(&mut app, "anything");
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.focus, Panel::HeadFiles);
    let _ = screen(&mut app);
}
