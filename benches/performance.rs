//! Run with `cargo bench --bench performance --locked`.
use ratatui::{Terminal, backend::TestBackend};
use rustifiedgit::{
    app::App,
    config::{Config, State},
    git::Repository,
    model::{Commit, FileEntry, Panel, RefKind, Reference, Snapshot},
    tree, ui,
};
use std::{collections::HashSet, process::Command, sync::Arc, time::Instant};

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
    println!(
        "{}",
        serde_json::json!({
            "files":100_000,"commits":100_000,"references":5000,
            "tree_build_ms": tree_samples, "app_rebuild_ms":rebuild_ms,
            "render_100_frames_ms":render_ms, "search_100k_commits_ms":search_ms,
        })
    );
}
