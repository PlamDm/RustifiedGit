use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use pd_lazygit::{
    app::App,
    config::{Config, State},
    git::Repository,
    model::Panel,
    ui,
};
use ratatui::{Terminal, backend::TestBackend, layout::Rect};
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
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn fixture() -> (tempfile::TempDir, App, String) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    for (key, value) in [
        ("user.name", "Graph Test"),
        ("user.email", "graph@example.invalid"),
        ("commit.gpgsign", "false"),
        ("core.hooksPath", ".test-hooks"),
    ] {
        git(&root, &["config", key, value]);
    }
    fs::write(root.join("base.txt"), "base\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "Base commit"]);
    git(&root, &["switch", "-qc", "feature"]);
    fs::write(root.join("feature.txt"), "feature\n").unwrap();
    git(&root, &["add", "."]);
    git(
        &root,
        &["commit", "-qm", "Feature commit\n\nDetailed feature body."],
    );
    let feature = git(&root, &["rev-parse", "HEAD"]);
    git(
        &root,
        &["update-ref", "refs/remotes/origin/feature", &feature],
    );
    git(&root, &["switch", "-q", "main"]);
    fs::write(root.join("main.txt"), "main\n").unwrap();
    git(&root, &["add", "."]);
    git(
        &root,
        &["commit", "-qm", "Main commit\n\nDetailed main body."],
    );
    fs::write(root.join("main.txt"), "working change\n").unwrap();
    let config = Config::load(Some(dir.path().join("config")), &[]).unwrap();
    let mut app = App::new(
        Repository::discover(&root).unwrap(),
        config,
        State::default(),
    );
    wait(&mut app, |app| !app.loading);
    (dir, app, feature)
}

fn wait(app: &mut App, done: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        app.poll_workers();
        if done(app) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "workers timed out; notice: {}",
            app.notice
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn key(app: &mut App, code: KeyCode) {
    app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn screen(app: &mut App, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| ui::draw(frame, app)).unwrap();
    let buffer = terminal.backend().buffer();
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn numbered_panes_and_tab_order_cover_the_layout() {
    let (_dir, mut app, _) = fixture();
    for (number, panel) in [
        ('1', Panel::Status),
        ('2', Panel::Branches),
        ('3', Panel::RemoteBranches),
        ('4', Panel::Stash),
        ('5', Panel::Commits),
        ('6', Panel::Details),
        ('7', Panel::Files),
        ('0', Panel::Diff),
        ('8', Panel::Output),
    ] {
        key(&mut app, KeyCode::Char(number));
        assert_eq!(app.focus, panel);
        key(&mut app, KeyCode::Tab);
    }
    key(&mut app, KeyCode::Char('1'));
    for panel in [
        Panel::Branches,
        Panel::RemoteBranches,
        Panel::Stash,
        Panel::Commits,
        Panel::Details,
        Panel::Files,
        Panel::Diff,
        Panel::Output,
        Panel::Status,
    ] {
        key(&mut app, KeyCode::Tab);
        assert_eq!(app.focus, panel);
    }
    app.state.hide_command_log = true;
    let area = Rect::new(0, 0, 160, 48);
    let layout = ui::areas(area, &app);
    assert!(layout.local.right() <= layout.graph.x);
    assert!(layout.remote.y >= layout.local.bottom());
    assert!(layout.stash.y >= layout.remote.bottom());
    assert!(layout.details.x >= layout.graph.right());
    assert!(layout.files.y >= layout.details.bottom());
    assert!(layout.diff.y >= layout.graph.bottom());
    assert!(layout.graph.width > layout.local.width);
    let panes = [
        layout.repositories,
        layout.local,
        layout.remote,
        layout.stash,
        layout.graph,
        layout.details,
        layout.files,
        layout.diff,
    ];
    for (index, pane) in panes.iter().enumerate() {
        assert!(pane.width >= 3 && pane.height >= 3);
        for other in panes.iter().skip(index + 1) {
            assert!(pane.intersection(*other).is_empty());
        }
    }
    let text = screen(&mut app, 160, 48);
    for title in [
        "1 Repositories",
        "2 Local branches",
        "3 Remote branches",
        "4 Stashes",
        "5 Commit graph",
        "6 Commit details",
        "7 Changed files",
        "0 Diff",
    ] {
        assert!(text.contains(title), "Missing {title}:\n{text}");
    }
    key(&mut app, KeyCode::Char('6'));
    key(&mut app, KeyCode::Char('+'));
    assert!(app.fullscreen);
    let zoomed = ui::areas(area, &app);
    assert_eq!(zoomed.details.width, area.width);
    assert_eq!(zoomed.graph, Rect::default());
    key(&mut app, KeyCode::Char('+'));
    assert!(!app.fullscreen);
}

#[test]
fn branch_visibility_is_persisted_without_checkout_and_combines_unique_commits() {
    let (_dir, mut app, feature) = fixture();
    let head = git(&app.repo.root, &["rev-parse", "HEAD"]);
    let status = git(&app.repo.root, &["status", "--porcelain"]);
    assert!(
        !app.snapshot
            .commits
            .iter()
            .any(|commit| commit.oid == feature)
    );
    let local = app
        .refs_for(false)
        .iter()
        .position(|index| app.snapshot.references[*index].name == "feature")
        .unwrap();
    app.select_list_row(Panel::Branches, local);
    key(&mut app, KeyCode::Char(' '));
    assert!(app.busy.is_none());
    wait(&mut app, |app| !app.loading);
    assert!(
        app.snapshot
            .commits
            .iter()
            .any(|commit| commit.oid == feature)
    );
    assert_eq!(app.snapshot.commits.len(), 3);
    let remote = app
        .refs_for(true)
        .iter()
        .position(|index| app.snapshot.references[*index].name == "origin/feature")
        .unwrap();
    app.select_list_row(Panel::RemoteBranches, remote);
    key(&mut app, KeyCode::Char(' '));
    wait(&mut app, |app| !app.loading);
    assert_eq!(
        app.snapshot.commits.len(),
        3,
        "shared commits must not duplicate"
    );
    assert_eq!(
        app.selected(Panel::Branches),
        local,
        "independent branch cursors"
    );
    assert_eq!(git(&app.repo.root, &["rev-parse", "HEAD"]), head);
    assert_eq!(git(&app.repo.root, &["status", "--porcelain"]), status);
    let saved = State::load(&app.config.directory).unwrap();
    assert_eq!(saved.visible_branches, app.state.visible_branches);
    let mut reopened = App::new(app.repo.clone(), app.config.clone(), saved);
    wait(&mut reopened, |app| !app.loading);
    assert_eq!(reopened.options.visible_refs, app.options.visible_refs);
    assert_eq!(reopened.snapshot.commits.len(), 3);
    for panel in [Panel::Branches, Panel::RemoteBranches] {
        let refs = reopened.refs_for(panel == Panel::RemoteBranches);
        for (position, index) in refs.into_iter().enumerate() {
            if reopened.branch_visible(&reopened.snapshot.references[index]) {
                reopened.select_list_row(panel, position);
                key(&mut reopened, KeyCode::Char(' '));
                wait(&mut reopened, |app| !app.loading);
            }
        }
    }
    assert_eq!(reopened.options.visible_refs, Some(vec![]));
    assert!(reopened.snapshot.commits.is_empty());
    assert!(reopened.graph_working_tree);
    assert!(screen(&mut reopened, 140, 40).contains("Working tree / Index"));
}

#[test]
fn graph_navigation_loads_message_files_and_diff_without_stealing_focus() {
    let (_dir, mut app, _) = fixture();
    key(&mut app, KeyCode::Char('j'));
    assert_eq!(app.focus, Panel::Commits);
    assert!(!app.graph_working_tree);
    let oid = app.selected_commit().unwrap().oid.clone();
    wait(&mut app, |app| {
        !app.inspection_loading && !app.preview_loading
    });
    assert_eq!(app.focus, Panel::Commits);
    assert_eq!(app.commit_details.as_ref().unwrap().oid, oid);
    assert!(
        app.details_lines
            .iter()
            .any(|line| line == "Detailed main body.")
    );
    assert_eq!(app.commit_files.len(), 1);
    assert_eq!(app.commit_files[0].path, Path::new("main.txt"));
    assert!(app.preview.text.contains("+main"));
    let index = app.filtered_commits()[app.selected(Panel::Commits)];
    std::sync::Arc::make_mut(&mut app.snapshot).commits[index].decorations =
        "HEAD -> main, origin/a-very-long-remote-branch-that-crowds-out-the-subject".into();
    let text = screen(&mut app, 140, 40);
    assert!(text.contains("Detailed main body."));
    let rect = ui::areas(Rect::new(0, 0, 140, 40), &app).graph;
    let commit_row: String = text
        .lines()
        .nth((rect.y + 2) as usize)
        .unwrap()
        .chars()
        .skip(rect.x as usize)
        .take(rect.width as usize)
        .collect();
    assert!(commit_row.contains("Main commit"), "{commit_row}");
    key(&mut app, KeyCode::Char('7'));
    assert_eq!(app.focus, Panel::CommitFiles);
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.focus, Panel::Diff);
    assert!(app.fullscreen);
    let layout = ui::areas(Rect::new(0, 0, 140, 40), &app);
    assert_eq!(layout.diff.width, 140);
    assert!(layout.graph.is_empty());
    assert!(layout.files.is_empty());
    key(&mut app, KeyCode::Esc);
    assert_eq!(app.focus, Panel::CommitFiles);
    assert!(!app.fullscreen);
    key(&mut app, KeyCode::Char('5'));
    key(&mut app, KeyCode::Home);
    wait(&mut app, |app| !app.preview_loading);
    assert!(app.graph_working_tree);
    assert!(app.commit_details.is_none());
    assert!(app.commit_rows.is_empty());
    assert_eq!(app.file_panel(), Panel::Files);
    assert!(app.preview.text.contains("+working change"));
    key(&mut app, KeyCode::Char('7'));
    let selected = app.selected(Panel::Files);
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.focus, Panel::Diff);
    assert!(app.fullscreen);
    key(&mut app, KeyCode::Esc);
    assert_eq!(app.focus, Panel::Files);
    assert_eq!(app.selected(Panel::Files), selected);
    assert!(!app.fullscreen);
}

#[test]
fn rapid_graph_navigation_discards_stale_inspections_and_preserves_selected_oid_on_refresh() {
    let (_dir, mut app, _) = fixture();
    app.select_graph_row(1);
    app.select_graph_row(2);
    app.select_graph_row(1);
    let oid = app.selected_commit().unwrap().oid.clone();
    wait(&mut app, |app| !app.inspection_loading);
    assert_eq!(app.commit_details.as_ref().unwrap().oid, oid);
    git(&app.repo.root, &["add", "main.txt"]);
    git(&app.repo.root, &["commit", "-qm", "New top commit"]);
    app.loading = false;
    app.refresh();
    wait(&mut app, |app| !app.loading && !app.inspection_loading);
    assert_eq!(app.selected_commit().unwrap().oid, oid);
    assert_eq!(app.selected(Panel::Commits), 1);
    assert_eq!(app.commit_details.as_ref().unwrap().oid, oid);
    app.select_graph_row(2);
    app.select_graph_row(0);
    wait(&mut app, |app| !app.preview_loading);
    std::thread::sleep(Duration::from_millis(30));
    app.poll_workers();
    assert!(app.commit_details.is_none());
    assert!(app.commit_files.is_empty());
    assert!(!app.inspection_loading);
    assert_eq!(app.focus, Panel::Commits);
}

#[test]
fn graph_mouse_rows_skip_connectors_and_use_visual_scroll_offsets() {
    let (_dir, mut app, _) = fixture();
    git(&app.repo.root, &["add", "main.txt"]);
    git(&app.repo.root, &["commit", "-qm", "Save work"]);
    for _ in 0..8 {
        git(
            &app.repo.root,
            &["commit", "--allow-empty", "-qm", "Graph padding"],
        );
    }
    git(
        &app.repo.root,
        &["merge", "--no-ff", "-qm", "Merge feature", "feature"],
    );
    app.refresh();
    wait(&mut app, |app| !app.loading);
    screen(&mut app, 160, 48);
    let rect = ui::areas(Rect::new(0, 0, 160, 48), &app).graph;
    let mut row = 1;
    for (logical, index) in app.filtered_commits().into_iter().enumerate() {
        let commit = &app.snapshot.commits[index];
        for _ in &commit.graph_before {
            assert_eq!(
                ui::hit_test(Rect::new(0, 0, 160, 48), &app, rect.x + 2, rect.y + 1 + row),
                None
            );
            row += 1;
        }
        assert_eq!(
            ui::hit_test(Rect::new(0, 0, 160, 48), &app, rect.x + 2, rect.y + 1 + row),
            Some((Panel::Commits, logical + 1))
        );
        row += 1;
    }
    assert!(
        app.snapshot
            .commits
            .iter()
            .any(|commit| !commit.graph_before.is_empty())
    );
    app.select_graph_row(app.snapshot.commits.len());
    screen(&mut app, 100, 22);
    let rect = ui::areas(Rect::new(0, 0, 100, 22), &app).graph;
    assert!(app.scrolls[3] > 0);
    let (_, logical) = ui::hit_test(
        Rect::new(0, 0, 100, 22),
        &app,
        rect.x + 2,
        rect.bottom() - 2,
    )
    .unwrap();
    assert_eq!(logical, app.snapshot.commits.len());
}

#[test]
fn details_scroll_uses_wrapped_rows_and_clamps_when_zoomed() {
    let (_dir, mut app, _) = fixture();
    app.set_focus(Panel::Details);
    app.details_lines = vec![
        "Wrapped message words ".repeat(50),
        "LAST_MESSAGE_LINE".into(),
    ];
    screen(&mut app, 100, 30);
    assert!(app.details_max_scroll > app.details_lines.len());
    key(&mut app, KeyCode::End);
    assert_eq!(app.scrolls[9], app.details_max_scroll);
    assert!(screen(&mut app, 100, 30).contains("LAST_MESSAGE_LINE"));
    key(&mut app, KeyCode::Char('+'));
    screen(&mut app, 100, 30);
    assert_eq!(app.scrolls[9], app.details_max_scroll);
    key(&mut app, KeyCode::Home);
    assert_eq!(app.scrolls[9], 0);
    assert_eq!(app.focus, Panel::Details);
}

#[test]
fn a_single_large_hunk_scrolls_immediately_and_can_still_be_staged_whole() {
    let (_dir, mut app, _) = fixture();
    let contents: String = (1..=200)
        .map(|line| format!("large-hunk-line-{line:04}\n"))
        .collect();
    fs::write(app.repo.root.join("main.txt"), &contents).unwrap();
    app.refresh();
    wait(&mut app, |app| !app.loading);
    key(&mut app, KeyCode::Char('7'));
    key(&mut app, KeyCode::Enter);
    wait(&mut app, |app| !app.preview_loading);
    assert!(app.fullscreen && app.hunk_mode);
    assert_eq!(app.patch.hunks.len(), 1);
    screen(&mut app, 120, 30);
    assert_eq!(app.diff_scroll, 0);

    key(&mut app, KeyCode::Down);
    screen(&mut app, 120, 30);
    assert_eq!(
        app.diff_scroll, 1,
        "one Down must scroll immediately inside a hunk"
    );
    assert_eq!(app.patch.hunk_at(app.diff_line), Some(0));
    key(&mut app, KeyCode::Up);
    screen(&mut app, 120, 30);
    assert_eq!(app.diff_scroll, 0);

    let page = app.diff_view_height - 1;
    key(&mut app, KeyCode::PageDown);
    screen(&mut app, 120, 30);
    assert_eq!(app.diff_scroll, page);
    key(&mut app, KeyCode::PageUp);
    screen(&mut app, 120, 30);
    assert_eq!(app.diff_scroll, 0);

    // Line selection keeps a moving cursor; page keys scroll in either mode.
    key(&mut app, KeyCode::Home);
    key(&mut app, KeyCode::Char('a'));
    key(&mut app, KeyCode::Down);
    screen(&mut app, 120, 30);
    assert!(!app.hunk_mode);
    assert_eq!(app.diff_line, 1);
    assert_eq!(app.diff_scroll, 0);
    key(&mut app, KeyCode::PageDown);
    screen(&mut app, 120, 30);
    assert_eq!(app.diff_scroll, page);
    key(&mut app, KeyCode::Char('a'));
    key(&mut app, KeyCode::Home);

    let mut rendered = String::new();
    for _ in 0..app.preview_lines.len() + 10 {
        key(&mut app, KeyCode::Down);
        rendered = screen(&mut app, 120, 30);
    }
    assert!(rendered.contains("+large-hunk-line-0200"), "{rendered}");
    assert_eq!(app.diff_line, app.preview_lines.len() - 1);
    assert_eq!(
        app.diff_scroll,
        app.preview_lines.len() - app.diff_view_height
    );
    let bottom = app.diff_scroll;
    key(&mut app, KeyCode::Up);
    screen(&mut app, 120, 30);
    assert_eq!(app.diff_scroll, bottom - 1);
    key(&mut app, KeyCode::End);
    assert!(screen(&mut app, 100, 22).contains("+large-hunk-line-0200"));
    key(&mut app, KeyCode::PageUp);
    screen(&mut app, 100, 22);
    assert_eq!(app.patch.hunk_at(app.diff_line), Some(0));
    key(&mut app, KeyCode::Char(' '));
    wait(&mut app, |app| app.busy.is_none() && !app.loading);
    assert_eq!(
        git(&app.repo.root, &["show", ":main.txt"]),
        contents.trim_end()
    );
    assert!(git(&app.repo.root, &["diff", "--", "main.txt"]).is_empty());
}
