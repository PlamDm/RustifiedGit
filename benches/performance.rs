//! Run with `cargo bench --bench performance --locked`.
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend};
use rustifiedgit::{
    app::App,
    config::{Config, GuiConfig, State},
    git::Repository,
    model::{Commit, FileEntry, Panel, PreviewTarget, RefKind, Reference, Snapshot},
    process::Cancellation,
    tree, ui,
};
use std::{collections::HashSet, fs, path::Path, process::Command, sync::Arc, time::Instant};

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().into()
}

fn inspection_samples(root: &Path) -> (Vec<f64>, Vec<f64>) {
    for (key, value) in [
        ("user.name", "Performance Fixture"),
        ("user.email", "fixture@example.invalid"),
        ("commit.gpgsign", "false"),
        ("core.hooksPath", ".benchmark-hooks"),
    ] {
        git(root, &["config", key, value]);
    }
    let paths: Vec<_> = (0..100)
        .map(|index| format!("inspection-{index:03}.txt"))
        .collect();
    for path in &paths {
        fs::write(root.join(path), "base\n").unwrap();
    }
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "Base"]);
    for path in &paths {
        fs::write(root.join(path), "changed\n").unwrap();
    }
    git(root, &["commit", "-qam", "Inspection fixture"]);
    let oid = git(root, &["rev-parse", "HEAD"]);
    let repository = Repository::discover(root).unwrap();
    let target = PreviewTarget::CommitFiles {
        oid: oid.clone(),
        paths: paths.into_iter().map(Into::into).collect(),
    };
    let mut previews = Vec::new();
    let mut inspections = Vec::new();
    for _ in 0..5 {
        let start = Instant::now();
        let preview = repository
            .preview(
                &target,
                false,
                &GuiConfig::default(),
                3,
                false,
                &Cancellation::default(),
            )
            .unwrap();
        previews.push(start.elapsed().as_secs_f64() * 1000.0);
        assert!(!preview.suppressed);
        assert!(preview.text.contains("+changed"));
        let start = Instant::now();
        let inspected = repository
            .inspect_commit(&oid, &Cancellation::default())
            .unwrap();
        inspections.push(start.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(inspected.files.len(), 100);
    }
    (previews, inspections)
}

fn main() {
    let directory = tempfile::tempdir().unwrap();
    assert!(
        Command::new("git")
            .args(["init", "-qb", "main"])
            .current_dir(directory.path())
            .status()
            .unwrap()
            .success()
    );
    let files: Vec<_> = (0..100_000)
        .map(|index| FileEntry {
            path: format!("group-{:04}/nested/file-{index:06}.rs", index % 1000).into(),
            old_path: None,
            index: b' ',
            worktree: if index % 37 == 0 { b'M' } else { b' ' },
        })
        .collect();
    let mut tree_samples = Vec::new();
    for _ in 0..3 {
        let start = Instant::now();
        let rows = tree::rows(&files, true, true, &HashSet::new(), "", false);
        tree_samples.push(start.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(
            rows.iter().filter(|row| !row.directory).count(),
            files.len()
        );
    }
    let collapsed: HashSet<_> = files
        .iter()
        .filter_map(|file| file.path.components().next())
        .map(|component| std::path::PathBuf::from(component.as_os_str()))
        .collect();
    let mut collapsed_tree_samples = Vec::new();
    for _ in 0..3 {
        let start = Instant::now();
        let rows = tree::rows(&files, true, true, &collapsed, "", false);
        collapsed_tree_samples.push(start.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(rows.len(), collapsed.len());
    }
    let config = Config::load(Some(directory.path().join("config")), &[]).unwrap();
    let mut app = App::new(
        Repository::discover(directory.path()).unwrap(),
        config,
        State::default(),
    );
    app.workers.refresh.cancel();
    app.workers.history.cancel();
    app.workers.preview.cancel();
    app.config.gui.show_unchanged_files = true;
    app.changed_only = false;
    app.focus = Panel::Files;
    app.side = Panel::Files;
    app.loading = false;
    app.snapshot = Arc::new(Snapshot {
        files,
        commits: Arc::new(
            (0..100_000)
                .map(|index| Commit {
                    oid: format!("{index:040x}"),
                    short: format!("{index:08x}"),
                    subject: format!("Improve parser and file rendering {index}"),
                    author: "Example Author".into(),
                    committer: "Example Author".into(),
                    committed_at: "2026-11-12T09:00:00+00:00".into(),
                    ..Commit::default()
                })
                .collect(),
        ),
        references: (0..5000)
            .map(|index| Reference {
                name: format!("feature/group-{}/branch-{index}", index % 20),
                full_name: format!("refs/heads/feature/group-{}/branch-{index}", index % 20),
                kind: RefKind::Branch,
                oid: format!("{index:040x}"),
                ..Reference::default()
            })
            .collect(),
        head: format!("{:040x}", 0),
        ..Snapshot::default()
    });
    let start = Instant::now();
    app.rebuild();
    let rebuild_ms = start.elapsed().as_secs_f64() * 1000.0;
    let mut terminal = Terminal::new(TestBackend::new(160, 42)).unwrap();
    terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
    let start = Instant::now();
    for index in 0..100 {
        app.selections[1] = index;
        terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
    }
    let render_ms = start.elapsed().as_secs_f64() * 1000.0;
    let start = Instant::now();
    app.searches[3] = "parser".into();
    assert_eq!(app.filtered_commits().len(), 100_000);
    let search_ms = start.elapsed().as_secs_f64() * 1000.0;
    app.searches[3].clear();
    let start = Instant::now();
    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
    app.workers.preview.cancel();
    assert_eq!(app.file_selected_count(), 100_000);
    terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
    let select_all_first_frame_ms = start.elapsed().as_secs_f64() * 1000.0;
    let start = Instant::now();
    for index in 0..100 {
        app.selections[1] = index;
        terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
    }
    let selected_render_ms = start.elapsed().as_secs_f64() * 1000.0;
    app.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
    let start = Instant::now();
    for _ in 0..50 {
        terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
    }
    let help_render_ms = start.elapsed().as_secs_f64() * 1000.0;
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    app.focus = Panel::Diff;
    app.fullscreen = true;
    app.hunk_mode = false;
    app.preview_lines = vec![format!("+{}", "a".repeat(200_000)); 30];
    let start = Instant::now();
    for index in 0..50 {
        app.horizontal_scroll = index % 5;
        terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
    }
    let long_diff_render_ms = start.elapsed().as_secs_f64() * 1000.0;
    let (selected_preview_ms, commit_inspection_ms) = inspection_samples(directory.path());
    println!(
        "{}",
        serde_json::json!({
            "files":100_000,"commits":100_000,"references":5000,
            "tree_build_ms": tree_samples, "app_rebuild_ms":rebuild_ms,
            "render_100_frames_ms":render_ms, "search_100k_commits_ms":search_ms,
            "collapsed_tree_build_ms":collapsed_tree_samples,
            "select_all_first_frame_ms":select_all_first_frame_ms,
            "selected_render_100_frames_ms":selected_render_ms,
            "help_render_50_frames_ms":help_render_ms,
            "long_diff_render_50_frames_ms":long_diff_render_ms,
            "selected_100_file_preview_ms":selected_preview_ms,
            "commit_inspection_ms":commit_inspection_ms,
        })
    );
}
