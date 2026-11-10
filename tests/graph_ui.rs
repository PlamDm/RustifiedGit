use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend, layout::Rect};
use rustifiedgit::{
    app::App,
    config::{Config, State},
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
        if done(app) && !app.history_loading {
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
fn keyboard_help_searches_immediately_and_never_runs_typed_shortcuts() {
    use rustifiedgit::app::Modal;
    let (_dir, mut app, _) = fixture();
    let head = git(&app.repo.root, &["rev-parse", "HEAD"]);
    let status = git(&app.repo.root, &["status", "--porcelain"]);
    key(&mut app, KeyCode::Char('?'));
    let rendered = screen(&mut app, 120, 36);
    for label in [
        "Keyboard help",
        "Type to search",
        "Shortcut",
        "Command",
        "Pane / topic",
    ] {
        assert!(rendered.contains(label), "{rendered}");
    }
    for character in "stash".chars() {
        key(&mut app, KeyCode::Char(character));
    }
    let Modal::Help { state } = app.modal.as_ref().unwrap() else {
        panic!("help closed")
    };
    assert_eq!(state.query, "stash");
    assert!(
        state
            .matches()
            .iter()
            .any(|index| state.entries[*index].label == "Inspect stashed files")
    );
    key(&mut app, KeyCode::PageDown);
    let Modal::Help { state } = app.modal.as_ref().unwrap() else {
        unreachable!()
    };
    assert_eq!(
        state.selected,
        (state.page_size - 1).min(state.matches().len() - 1)
    );
    for character in "qjk?".chars() {
        key(&mut app, KeyCode::Char(character));
    }
    key(&mut app, KeyCode::Enter);
    let Modal::Help { state } = app.modal.as_ref().unwrap() else {
        panic!("text executed")
    };
    assert_eq!(state.query, "stashqjk?");
    assert!(!app.quit);
    assert!(app.busy.is_none());
    assert_eq!(git(&app.repo.root, &["rev-parse", "HEAD"]), head);
    assert_eq!(git(&app.repo.root, &["status", "--porcelain"]), status);
    key(&mut app, KeyCode::Esc);
    assert!(app.modal.is_none());
    key(&mut app, KeyCode::Char('?'));
    let Modal::Help { state } = app.modal.as_ref().unwrap() else {
        unreachable!()
    };
    assert!(state.query.is_empty());
    assert_eq!(state.selected, 0);
}

#[test]
fn keyboard_help_handles_paste_empty_results_highlights_and_small_screens() {
    use ratatui::style::Modifier;
    use rustifiedgit::app::Modal;
    let (_dir, mut app, _) = fixture();
    key(&mut app, KeyCode::Char('?'));
    app.paste("cherry-pick selected files".into());
    let text = screen(&mut app, 120, 36);
    assert!(text.contains("Cherry-pick selected files"), "{text}");
    let mut terminal = Terminal::new(TestBackend::new(120, 36)).unwrap();
    terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
    assert!(
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .any(|cell| cell.modifier.contains(Modifier::UNDERLINED))
    );
    app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    app.paste("zzzz-no-such-command-🦀\n".into());
    let text = screen(&mut app, 120, 36);
    assert!(text.contains("No matching commands"), "{text}");
    key(&mut app, KeyCode::End);
    let Modal::Help { state } = app.modal.as_ref().unwrap() else {
        unreachable!()
    };
    assert_eq!(state.selected, 0);
    assert!(!state.query.contains('\n'));
    app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    for (width, height) in [(120, 36), (80, 24), (40, 10), (20, 6), (1, 1)] {
        screen(&mut app, width, height);
        key(&mut app, KeyCode::End);
        screen(&mut app, width, height);
        key(&mut app, KeyCode::Home);
    }
}

#[test]
fn keyboard_help_includes_new_commands_remaps_disabled_and_other_context_custom_commands() {
    use rustifiedgit::{app::Modal, config::CustomCommand};
    let (_dir, mut app, _) = fixture();
    app.config.keybinding = serde_yaml::from_str(
        "universal:\n  optionMenu: '<ctrl+h>'\n  pull: '<ctrl+p>'\nfiles:\n  commitChanges: ['<ctrl+g>', '<ctrl+j>']\ncommits:\n  pasteCommits: '<disabled>'\n"
    ).unwrap();
    app.config.custom_commands.push(CustomCommand {
        key: "z".into(),
        context: "stash".into(),
        description: "Archive stash externally".into(),
        command: "printf custom-stash-command".into(),
        ..CustomCommand::default()
    });
    app.handle_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL));
    let Modal::Help { state } = app.modal.as_ref().unwrap() else {
        panic!("remapped help did not open")
    };
    let entries = &state.entries;
    for label in [
        "Cherry-pick selected files…",
        "Inspect stashed files",
        "Compare with base branch",
        "Loaded commits…",
        "Changes / HEAD tree",
        "Force push with lease…",
        "Rebase + autostash",
        "Details / output…",
        "Archive stash externally",
    ] {
        assert!(
            entries.iter().any(|entry| entry.label == label),
            "missing: {label}"
        );
    }
    assert!(
        entries
            .iter()
            .any(|entry| entry.label == "Commit changes…" && entry.keys == "Ctrl+g / Ctrl+j")
    );
    assert!(
        entries
            .iter()
            .any(|entry| entry.label == "Cherry-pick copied commits…" && entry.keys == "Disabled")
    );
    assert!(
        entries
            .iter()
            .any(|entry| entry.label == "Fast-forward only" && entry.keys == "Ctrl+p → 1")
    );
    assert!(
        entries
            .iter()
            .any(|entry| entry.label == "Archive stash externally"
                && entry.section == "Custom: stash")
    );
}

#[test]
fn browsing_stashes_updates_file_pane_and_keeps_it_active_after_refresh() {
    let (_dir, mut app, _) = fixture();
    let root = app.repo.root.clone();
    git(&root, &["stash", "push", "-qm", "main changes"]);
    fs::write(root.join("base.txt"), "saved base change\n").unwrap();
    git(&root, &["stash", "push", "-qm", "base changes"]);
    app.refresh();
    wait(&mut app, |app| !app.loading);

    key(&mut app, KeyCode::Char('4'));
    wait(&mut app, |app| !app.stash_inspection_loading());
    assert_eq!(app.file_panel(), Panel::CommitFiles);
    assert!(
        app.file_rows(Panel::CommitFiles)
            .iter()
            .any(|row| row.path == Path::new("base.txt"))
    );
    key(&mut app, KeyCode::Down);
    wait(&mut app, |app| !app.stash_inspection_loading());
    assert!(
        app.file_rows(Panel::CommitFiles)
            .iter()
            .any(|row| row.path == Path::new("main.txt"))
    );
    assert!(
        !app.file_rows(Panel::CommitFiles)
            .iter()
            .any(|row| row.path == Path::new("base.txt"))
    );

    key(&mut app, KeyCode::Char('7'));
    assert_eq!(app.focus, Panel::CommitFiles);
    app.refresh();
    wait(&mut app, |app| !app.loading);
    assert_eq!(app.focus, Panel::CommitFiles);
    assert!(app.stash_inspection_active());
    key(&mut app, KeyCode::Enter);
    wait(&mut app, |app| !app.preview_loading);
    assert_eq!(app.focus, Panel::Diff);
    assert!(app.preview.text.contains("+working change"));
    assert!(app.preview.text.contains("-main"));

    key(&mut app, KeyCode::Char('5'));
    assert!(!app.stash_inspection_active());
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
    app.set_focus(Panel::Details);
    key(&mut app, KeyCode::Char('1'));
    for panel in [
        Panel::Branches,
        Panel::RemoteBranches,
        Panel::Stash,
        Panel::Commits,
        Panel::Details,
        Panel::Files,
        Panel::Diff,
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
        "6 [Details] | Output",
        "7 [Changes] | HEAD tree",
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
        .branch_rows(false)
        .iter()
        .position(|row| {
            row.reference
                .is_some_and(|index| app.snapshot.references[index].name == "feature")
        })
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
        .branch_rows(true)
        .iter()
        .position(|row| {
            row.reference
                .is_some_and(|index| app.snapshot.references[index].name == "origin/feature")
        })
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
        for index in refs {
            if reopened.branch_visible(&reopened.snapshot.references[index]) {
                let position = reopened
                    .branch_rows(panel == Panel::RemoteBranches)
                    .iter()
                    .position(|row| row.reference == Some(index))
                    .unwrap();
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
    let snapshot = std::sync::Arc::make_mut(&mut app.snapshot);
    std::sync::Arc::make_mut(&mut snapshot.commits)[index].decorations =
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
    assert!(commit_row.contains("[main, origin/"), "{commit_row}");
    assert!(!commit_row.contains("Main commit"), "{commit_row}");
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
fn hovered_commit_details_list_local_remote_branches_and_tags() {
    let (_dir, mut app, feature) = fixture();
    git(&app.repo.root, &["tag", "v1.0", &feature]);
    app.options.visible_refs = Some(vec!["refs/heads/main".into(), "refs/heads/feature".into()]);
    app.refresh();
    wait(&mut app, |app| !app.loading);
    let filtered_index = app
        .filtered_commits()
        .iter()
        .position(|index| app.snapshot.commits[*index].oid == feature)
        .unwrap();
    app.select_graph_row(app.commit_graph_row(filtered_index));
    wait(&mut app, |app| !app.inspection_loading);
    assert!(
        app.details_lines
            .iter()
            .any(|line| line == "Local branches: feature")
    );
    assert!(
        app.details_lines
            .iter()
            .any(|line| line == "Remote branches: origin/feature")
    );
    assert!(app.details_lines.iter().any(|line| line == "Tags: v1.0"));
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

#[test]
fn history_prefetches_past_the_first_page_and_status_refreshes_reuse_it() {
    use rustifiedgit::git::HISTORY_PAGE_SIZE;
    use std::{io::Write, process::Stdio, sync::Arc};

    let (_dir, mut app, _) = fixture();
    let count = HISTORY_PAGE_SIZE + 405;
    let mut stream = String::from("blob\nmark :1\ndata 5\npage\n\n");
    for index in 0..count {
        stream.push_str(&format!(
            "commit refs/heads/main\nmark :{}\ncommitter Graph Test <graph@example.invalid> {} +0000\ndata {}\nPage commit {}\n",
            index + 2, 1700000000 + index, format!("Page commit {index}").len() + 1, index
        ));
        if index == 0 {
            stream.push_str("from refs/heads/main^0\n");
        } else {
            stream.push_str(&format!("from :{}\n", index + 1));
        }
        stream.push_str("M 100644 :1 history.txt\n\n");
    }
    let mut child = Command::new("git")
        .args(["fast-import", "--quiet"])
        .current_dir(&app.repo.root)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stream.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    app.refresh();
    wait(&mut app, |app| !app.loading);
    assert_eq!(app.snapshot.commits.len(), HISTORY_PAGE_SIZE);
    assert!(!app.history_exhausted);
    let status_generation = app.workers.refresh.current();
    app.select_graph_row(HISTORY_PAGE_SIZE - 25);
    let oid = app.selected_commit().unwrap().oid.clone();
    assert!(app.history_loading);
    wait(&mut app, |app| !app.inspection_loading);
    assert_eq!(app.snapshot.commits.len(), count + 2);
    assert!(app.history_exhausted);
    assert_eq!(app.selected_commit().unwrap().oid, oid);
    assert_eq!(
        app.workers.refresh.current(),
        status_generation,
        "paging must not scan the worktree"
    );
    let unique: std::collections::HashSet<_> = app
        .snapshot
        .commits
        .iter()
        .map(|commit| &commit.oid)
        .collect();
    assert_eq!(unique.len(), count + 2);

    let history_generation = app.workers.history.current();
    let cached = app.snapshot.commits.clone();
    fs::write(app.repo.root.join("only-status.txt"), "dirty\n").unwrap();
    app.refresh();
    wait(&mut app, |app| !app.loading);
    assert_eq!(
        app.workers.history.current(),
        history_generation,
        "unchanged refs must reuse history"
    );
    assert!(Arc::ptr_eq(&cached, &app.snapshot.commits));
    assert!(
        app.snapshot
            .files
            .iter()
            .any(|file| file.path == Path::new("only-status.txt"))
    );
}

#[test]
fn history_acceleration_builds_only_git_metadata() {
    let (_dir, mut app, _) = fixture();
    let head = git(&app.repo.root, &["rev-parse", "HEAD"]);
    let status = git(&app.repo.root, &["status", "--porcelain"]);
    let index = fs::read(app.repo.git_dir.join("index")).unwrap();
    key(&mut app, KeyCode::Char('G'));
    assert!(app.busy.is_some());
    wait(&mut app, |app| app.busy.is_none() && !app.loading);
    assert!(app.repo.git_dir.join("objects/info/commit-graph").exists());
    assert_eq!(git(&app.repo.root, &["rev-parse", "HEAD"]), head);
    assert_eq!(git(&app.repo.root, &["status", "--porcelain"]), status);
    assert_eq!(fs::read(app.repo.git_dir.join("index")).unwrap(), index);
}

#[test]
fn commit_rows_reserve_the_right_edge_for_committer_initials_and_timestamp() {
    use std::sync::Arc;
    let (_dir, mut app, _) = fixture();
    let snapshot = Arc::make_mut(&mut app.snapshot);
    let commit = &mut Arc::make_mut(&mut snapshot.commits)[0];
    commit.author = "Other Author".into();
    commit.date = "1999-02-01T04:07:58Z".into();
    commit.committer = "Jane Mary Smith".into();
    commit.committed_at = "2026-11-26T12:34:56+02:00".into();
    commit.subject = "修复 Unicode columns and a very long commit message ".repeat(8);
    for (width, height) in [(80, 24), (120, 34), (180, 46)] {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
        let rect = ui::areas(Rect::new(0, 0, width, height), &app).graph;
        let buffer = terminal.backend().buffer();
        let line: String = (rect.x + 1..rect.right() - 1)
            .map(|x| buffer[(x, rect.y + 2)].symbol())
            .collect();
        assert!(line.ends_with("JS 26 Nov 2026 12:34"), "{width}: {line}");
        assert!(line.contains('…'), "{width}: {line}");
        assert!(!line.contains("1999"));
    }
    let snapshot = Arc::make_mut(&mut app.snapshot);
    let commit = &mut Arc::make_mut(&mut snapshot.commits)[0];
    commit.committer.clear();
    commit.committed_at.clear();
    let text = screen(&mut app, 140, 40);
    assert!(text.contains("OA 01 Feb 1999 04:07"), "{text}");
}

#[test]
fn commit_rows_prioritize_all_branch_and_tag_labels_over_the_subject() {
    let (_dir, mut app, _) = fixture();
    git(&app.repo.root, &["branch", "release/stable"]);
    git(
        &app.repo.root,
        &["update-ref", "refs/remotes/origin/main", "HEAD"],
    );
    git(&app.repo.root, &["tag", "v1.0"]);
    git(&app.repo.root, &["tag", "-am", "Release", "v2.0"]);
    app.refresh();
    wait(&mut app, |app| !app.loading);
    let snapshot = std::sync::Arc::make_mut(&mut app.snapshot);
    let commit = &mut std::sync::Arc::make_mut(&mut snapshot.commits)[0];
    commit.subject = "A very long commit subject ".repeat(20);
    app.fullscreen = true;
    for width in [140, 80] {
        let text = screen(&mut app, width, 40);
        let rect = ui::areas(Rect::new(0, 0, width, 40), &app).graph;
        let line = text.lines().nth((rect.y + 2) as usize).unwrap();
        assert!(line.contains("[main,"), "{width}: {line}");
        assert!(line.contains('…'), "{width}: {line}");
        if width == 140 {
            for label in ["release/stable", "origin/main", "tag: v1.0", "tag: v2.0"] {
                assert!(line.contains(label), "{width}: missing {label}: {line}");
            }
            assert!(line.contains("] A very long"), "{line}");
        } else {
            assert!(line.contains("…]"), "{line}");
            assert!(!line.contains("A very long"), "{line}");
        }
    }
}

#[test]
fn commit_file_selection_previews_cherry_picks_and_discards_only_selected_paths() {
    let (_dir, mut app, _) = fixture();
    let root = app.repo.root.clone();
    git(&root, &["config", "core.autocrlf", "false"]);
    let head = git(&root, &["rev-parse", "HEAD"]);
    git(&root, &["switch", "-qc", "pick-files"]);
    fs::create_dir_all(root.join("nested/deep")).unwrap();
    fs::write(root.join("nested/deep/a.txt"), "picked a\n").unwrap();
    fs::write(root.join("nested/deep/b.txt"), "picked b\n").unwrap();
    fs::write(root.join("unpicked.txt"), "must not be picked\n").unwrap();
    git(&root, &["add", "--all"]);
    git(&root, &["commit", "-qm", "Selected file source"]);
    let source = git(&root, &["rev-parse", "HEAD"]);
    git(&root, &["switch", "-q", "main"]);
    fs::write(root.join("main.txt"), "keep working changes\n").unwrap();
    fs::write(root.join("keep.txt"), "keep untracked\n").unwrap();
    app.options.visible_refs = Some(vec![
        "refs/heads/main".into(),
        "refs/heads/pick-files".into(),
    ]);
    app.refresh();
    wait(&mut app, |app| !app.loading);
    let selected = app
        .filtered_commits()
        .iter()
        .position(|index| app.snapshot.commits[*index].oid == source)
        .unwrap();
    app.select_graph_row(app.commit_graph_row(selected));
    wait(&mut app, |app| !app.inspection_loading);
    key(&mut app, KeyCode::Char('7'));
    let folder = app
        .commit_rows
        .iter()
        .position(|row| row.path == Path::new("nested"))
        .unwrap();
    app.select_list_row(Panel::CommitFiles, folder);
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT));
    assert_eq!(app.marked_commit_files.len(), 2);
    assert!(app.file_is_marked(Path::new("nested/deep/a.txt")));
    wait(&mut app, |app| !app.preview_loading);
    assert!(app.preview.text.contains("+picked a"));
    assert!(app.preview.text.contains("+picked b"));
    assert!(!app.preview.text.contains("must not be picked"));
    let text = screen(&mut app, 160, 45);
    assert!(text.contains("●"), "{text}");
    key(&mut app, KeyCode::Esc);
    assert!(app.marked_commit_files.is_empty());
    assert_eq!(app.focus, Panel::CommitFiles);
    app.select_list_row(Panel::CommitFiles, folder);
    key(&mut app, KeyCode::Char('*'));
    assert!(app.collapsed.contains(Path::new("nested")));
    assert!(app.collapsed.contains(Path::new("nested/deep")));
    assert!(
        !app.commit_rows
            .iter()
            .any(|row| row.path == Path::new("nested/deep/a.txt"))
    );
    key(&mut app, KeyCode::Char('*'));
    assert!(
        app.commit_rows
            .iter()
            .any(|row| row.path == Path::new("nested/deep/a.txt"))
    );
    app.select_list_row(Panel::CommitFiles, folder);
    key(&mut app, KeyCode::Char('v'));
    key(&mut app, KeyCode::Char('0'));
    key(&mut app, KeyCode::Esc);
    assert_eq!(app.focus, Panel::CommitFiles);
    assert!(app.marked_commit_files.is_empty());
    key(&mut app, KeyCode::Char('v'));
    key(&mut app, KeyCode::Char('V'));
    assert!(matches!(
        app.modal,
        Some(rustifiedgit::app::Modal::Confirm { .. })
    ));
    key(&mut app, KeyCode::Enter);
    wait(&mut app, |app| app.busy.is_none() && !app.loading);
    assert_eq!(git(&root, &["rev-parse", "HEAD"]), head);
    assert_eq!(
        fs::read_to_string(root.join("nested/deep/a.txt")).unwrap(),
        "picked a\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("nested/deep/b.txt")).unwrap(),
        "picked b\n"
    );
    assert!(!root.join("unpicked.txt").exists());
    assert_eq!(
        git(&root, &["diff", "--cached", "--name-only"]),
        "nested/deep/a.txt\nnested/deep/b.txt"
    );
    assert_eq!(
        fs::read_to_string(root.join("main.txt")).unwrap(),
        "keep working changes\n"
    );
    key(&mut app, KeyCode::Char('d'));
    assert!(matches!(
        app.modal,
        Some(rustifiedgit::app::Modal::Confirm { .. })
    ));
    key(&mut app, KeyCode::Enter);
    wait(&mut app, |app| app.busy.is_none() && !app.loading);
    assert!(!root.join("nested/deep/a.txt").exists());
    assert!(!root.join("nested/deep/b.txt").exists());
    assert_eq!(
        fs::read_to_string(root.join("main.txt")).unwrap(),
        "keep working changes\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("keep.txt")).unwrap(),
        "keep untracked\n"
    );
    assert!(git(&root, &["diff", "--cached", "--name-only"]).is_empty());
}

#[test]
fn nested_branch_groups_collapse_search_and_toggle_visibility_without_checkout() {
    let (_dir, mut app, _) = fixture();
    git(&app.repo.root, &["branch", "-D", "feature"]);
    git(
        &app.repo.root,
        &["update-ref", "-d", "refs/remotes/origin/feature"],
    );
    for branch in [
        "feature/user/login",
        "feature/user/profile",
        "feature/admin/tools",
        "bugfix/urgent",
    ] {
        git(&app.repo.root, &["branch", branch, "main"]);
    }
    for branch in ["origin/feature/user/login", "origin/feature/user/profile"] {
        git(
            &app.repo.root,
            &["update-ref", &format!("refs/remotes/{branch}"), "HEAD"],
        );
    }
    app.refresh();
    wait(&mut app, |app| !app.loading);
    let row = |app: &App, remote, path: &str| {
        app.branch_rows(remote)
            .iter()
            .position(|row| row.path == path)
            .unwrap()
    };
    assert_eq!(app.branch_rows(false)[row(&app, false, "feature")].depth, 0);
    assert_eq!(
        app.branch_rows(false)[row(&app, false, "feature/user")].depth,
        1
    );
    assert_eq!(
        app.branch_rows(false)[row(&app, false, "feature/user/login")].depth,
        2
    );
    assert_eq!(
        app.branch_rows(true)[row(&app, true, "origin/feature/user/login")].depth,
        3
    );
    let head = git(&app.repo.root, &["rev-parse", "HEAD"]);
    let status = git(&app.repo.root, &["status", "--porcelain"]);
    app.select_list_row(Panel::Branches, row(&app, false, "feature/user"));
    let children = app.selected_branch_row().unwrap().descendants.clone();
    key(&mut app, KeyCode::Char(' '));
    assert!(app.busy.is_none());
    wait(&mut app, |app| !app.loading);
    assert!(
        children
            .iter()
            .all(|index| app.branch_visible(&app.snapshot.references[*index]))
    );
    assert_eq!(git(&app.repo.root, &["rev-parse", "HEAD"]), head);
    assert_eq!(git(&app.repo.root, &["status", "--porcelain"]), status);
    key(&mut app, KeyCode::Enter);
    assert!(
        app.branch_rows(false)
            .iter()
            .all(|row| row.path != "feature/user/login")
    );
    assert_eq!(app.selected_branch_row().unwrap().path, "feature/user");
    for character in "/profile".chars() {
        key(&mut app, KeyCode::Char(character));
    }
    key(&mut app, KeyCode::Enter);
    assert!(
        app.branch_rows(false)
            .iter()
            .any(|row| row.path == "feature/user/profile")
    );
    key(&mut app, KeyCode::Esc);
    app.select_list_row(Panel::Branches, row(&app, false, "feature/user"));
    key(&mut app, KeyCode::Right);
    assert!(
        app.branch_rows(false)
            .iter()
            .any(|row| row.path == "feature/user/login")
    );
    key(&mut app, KeyCode::Right);
    assert_eq!(app.selected_ref().unwrap().name, "feature/user/login");
    key(&mut app, KeyCode::Left);
    assert_eq!(app.selected_branch_row().unwrap().path, "feature/user");
    app.select_list_row(
        Panel::RemoteBranches,
        row(&app, true, "origin/feature/user"),
    );
    key(&mut app, KeyCode::Char(' '));
    wait(&mut app, |app| !app.loading);
    assert!(
        app.selected_branch_row()
            .unwrap()
            .descendants
            .iter()
            .all(|index| app.branch_visible(&app.snapshot.references[*index]))
    );
    assert!(app.busy.is_none());
    assert_eq!(git(&app.repo.root, &["rev-parse", "HEAD"]), head);

    app.select_list_row(Panel::Branches, row(&app, false, "feature/user/login"));
    key(&mut app, KeyCode::Enter);
    wait(&mut app, |app| app.busy.is_none() && !app.loading);
    assert_eq!(
        git(&app.repo.root, &["symbolic-ref", "--short", "HEAD"]),
        "feature/user/login"
    );
}

#[test]
fn configured_branch_delimiters_support_nested_groups_and_flat_mode() {
    let (_dir, mut app, _) = fixture();
    git(&app.repo.root, &["branch", "topic--user--login"]);
    git(&app.repo.root, &["branch", "topic--user--profile"]);
    git(&app.repo.root, &["branch", "topic--user"]);
    app.config.gui.branch_group_delimiter = "--".into();
    app.refresh();
    wait(&mut app, |app| !app.loading);
    assert!(
        app.branch_rows(false)
            .iter()
            .any(|row| row.path == "topic--user" && row.depth == 1 && row.is_group())
    );
    assert!(
        app.branch_rows(false)
            .iter()
            .any(|row| row.path == "topic--user--login" && row.depth == 2 && !row.is_group())
    );
    let leaf = app
        .branch_rows(false)
        .iter()
        .position(|row| row.path == "topic--user" && !row.is_group())
        .unwrap();
    app.select_list_row(Panel::Branches, leaf);
    app.rebuild_branch_rows();
    assert_eq!(app.selected_ref().unwrap().name, "topic--user");
    app.config.gui.branch_group_delimiter.clear();
    app.rebuild_branch_rows();
    assert!(
        app.branch_rows(false)
            .iter()
            .all(|row| !row.is_group() && row.depth == 0)
    );
    std::sync::Arc::make_mut(&mut app.snapshot).worktrees = (0..3)
        .map(|index| rustifiedgit::model::Worktree {
            path: format!("worktree-{index}").into(),
            ..Default::default()
        })
        .collect();
    app.branch_tab = 3;
    app.selections[rustifiedgit::app::panel_index(Panel::Branches)] = 2;
    app.rebuild();
    assert_eq!(
        app.selected(Panel::Branches),
        2,
        "branch grouping must preserve worktree selection"
    );
}

#[test]
fn actions_menu_shows_focused_commands_and_runs_the_selected_action() {
    use rustifiedgit::app::Modal;
    let (_dir, mut app, _) = fixture();
    key(&mut app, KeyCode::Char('O'));
    let Some(Modal::Actions { items, .. }) = &app.modal else {
        panic!("Expected action menu");
    };
    assert!(
        !items.iter().any(|action| action.label == "Commit…"),
        "No staged files yet"
    );
    let index = items
        .iter()
        .position(|action| action.label == "Stage all")
        .unwrap();
    let description = items[index].description.clone();
    for _ in 0..index {
        key(&mut app, KeyCode::Down);
    }
    let text = screen(&mut app, 150, 42);
    assert!(text.contains(&description));
    key(&mut app, KeyCode::Enter);
    assert!(app.modal.is_none());
    assert!(app.busy.is_some());
    wait(&mut app, |app| app.busy.is_none() && !app.loading);
    assert_eq!(
        git(&app.repo.root, &["diff", "--cached", "--name-only"]),
        "main.txt"
    );

    key(&mut app, KeyCode::Char('7'));
    key(&mut app, KeyCode::Char('o'));
    assert!(
        app.pending_external.take().is_some(),
        "o must retain its file opener behavior"
    );
    assert!(app.modal.is_none());
    key(&mut app, KeyCode::Char('O'));
    assert!(matches!(app.modal, Some(Modal::Actions { .. })));
    key(&mut app, KeyCode::Esc);
    app.select_graph_row(1);
    wait(&mut app, |app| !app.inspection_loading);
    let head = git(&app.repo.root, &["rev-parse", "HEAD"]);
    key(&mut app, KeyCode::Char('o'));
    let Some(Modal::Actions { items, .. }) = &app.modal else {
        panic!("o should open graph actions");
    };
    let index = items
        .iter()
        .position(|action| action.label == "Reset options…")
        .unwrap();
    for _ in 0..index {
        key(&mut app, KeyCode::Down);
    }
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.modal, Some(Modal::Menu { .. })));
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Enter);
    assert!(
        matches!(app.modal, Some(Modal::Confirm { .. })),
        "Destructive actions must retain confirmation"
    );
    assert!(app.busy.is_none());
    assert_eq!(git(&app.repo.root, &["rev-parse", "HEAD"]), head);
}

#[test]
fn action_menu_respects_remaps_disabled_actions_and_custom_commands() {
    use rustifiedgit::{actions, app::Modal, config::CustomCommand};
    let (_dir, mut app, _) = fixture();
    app.config.keybinding = serde_yaml::from_str(
        "universal:\n  select: '<ctrl+g>'\nfiles:\n  toggleStagedAll: '<disabled>'\n",
    )
    .unwrap();
    app.config.custom_commands.push(CustomCommand {
        key: "x".into(),
        context: "files".into(),
        description: "Say hello".into(),
        command: "printf action-menu-ready".into(),
        ..CustomCommand::default()
    });
    key(&mut app, KeyCode::Char('7'));
    let available = actions::for_app(&app);
    assert!(!available.iter().any(|action| action.label == "Stage all"));
    assert_eq!(
        available
            .iter()
            .find(|action| action.label == "Stage / unstage")
            .unwrap()
            .key,
        "Ctrl+g"
    );
    key(&mut app, KeyCode::Char('O'));
    let Some(Modal::Actions { items, .. }) = &app.modal else {
        panic!("Expected actions");
    };
    assert!(
        items
            .iter()
            .any(|action| action.label == "Say hello" && action.key == "x")
    );
    app.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL));
    wait(&mut app, |app| app.busy.is_none() && !app.loading);
    assert_eq!(
        git(&app.repo.root, &["diff", "--cached", "--name-only"]),
        "main.txt"
    );
    key(&mut app, KeyCode::Char('O'));
    key(&mut app, KeyCode::Char('x'));
    wait(&mut app, |app| app.busy.is_none() && !app.loading);
    assert!(
        app.output
            .iter()
            .any(|line| line.contains("action-menu-ready"))
    );
}

#[test]
fn graph_fast_scrolling_tracks_visible_height_and_keeps_file_range_selection() {
    use rustifiedgit::model::{FileEntry, Snapshot};
    use std::{io::Write, process::Stdio, sync::Arc};
    let (_dir, mut app, _) = fixture();
    let parent = git(&app.repo.root, &["rev-parse", "HEAD"]);
    let mut stream = String::new();
    for index in 0..90 {
        let message = format!("Navigation commit {index}");
        stream.push_str(&format!("commit refs/heads/main\nmark :{}\ncommitter Graph Test <graph@example.invalid> {} +0000\ndata {}\n{message}\nfrom {}\n\n", index + 1, 1700000000 + index, message.len() + 1, if index == 0 { parent.clone() } else { format!(":{index}") }));
    }
    let mut child = Command::new("git")
        .args(["fast-import", "--quiet"])
        .current_dir(&app.repo.root)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stream.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    app.refresh();
    wait(&mut app, |app| !app.loading);
    screen(&mut app, 120, 30);
    let short_page = app.page_step();
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT));
    assert_eq!(app.selected(Panel::Commits) + 1, short_page);
    assert!(app.marked.is_empty());
    key(&mut app, KeyCode::Down);
    assert_eq!(app.selected(Panel::Commits) + 1, short_page + 1);
    key(&mut app, KeyCode::Home);
    screen(&mut app, 120, 55);
    let tall_page = app.page_step();
    assert!(tall_page > short_page);
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT));
    assert_eq!(app.selected(Panel::Commits) + 1, tall_page);
    app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT));
    assert!(app.graph_working_tree);
    key(&mut app, KeyCode::PageDown);
    assert_eq!(app.selected(Panel::Commits) + 1, tall_page);

    app.graph_working_tree = true;
    app.snapshot = Arc::new(Snapshot {
        files: ["a.txt", "b.txt"]
            .into_iter()
            .map(|path| FileEntry {
                path: path.into(),
                old_path: None,
                index: b' ',
                worktree: b'M',
            })
            .collect(),
        ..Snapshot::default()
    });
    app.rebuild();
    key(&mut app, KeyCode::Char('7'));
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT));
    assert_eq!(app.selected(Panel::Files), 1);
    assert_eq!(app.marked.len(), 2);
}
