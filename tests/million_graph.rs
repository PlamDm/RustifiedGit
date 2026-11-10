use ratatui::{Terminal, backend::TestBackend, layout::Rect};
use rustifiedgit::{
    app::App,
    config::{Config, State},
    git::Repository,
    history::{CommitOrder, GraphIndex, GraphRow},
    model::{Commit, Panel, Snapshot},
    ui,
};
use std::{process::Command, sync::Arc, time::Instant};

#[test]
fn compact_graph_rows_keep_connectors_and_worktree_mapping() {
    let commits = vec![
        Commit::default(),
        Commit {
            graph_before: vec!["|\\".into(), "|/".into()],
            ..Commit::default()
        },
        Commit::default(),
    ];
    let order = CommitOrder::new(vec![0, 1, 2]);
    let index = GraphIndex::new(order.clone(), &commits, false);
    assert_eq!(index.len(), 6);
    assert_eq!(
        index.row(0, 1),
        Some(GraphRow::Commit {
            commit_index: 0,
            logical: 0
        })
    );
    assert_eq!(
        index.row(1, 1),
        Some(GraphRow::Connector {
            commit_index: 1,
            line: 0
        })
    );
    assert_eq!(
        index.row(2, 1),
        Some(GraphRow::Connector {
            commit_index: 1,
            line: 1
        })
    );
    assert_eq!(
        index.row(3, 1),
        Some(GraphRow::WorkingTree {
            commit_index: Some(1),
            logical: 1
        })
    );
    assert_eq!(
        index.row(4, 1),
        Some(GraphRow::Commit {
            commit_index: 1,
            logical: 2
        })
    );
    assert_eq!(
        index.row(5, 1),
        Some(GraphRow::Commit {
            commit_index: 2,
            logical: 3
        })
    );
    for logical in 0..4 {
        let visual = index.visual_for_logical(logical, 1).unwrap();
        assert_eq!(index.row(visual, 1).unwrap().logical(), Some(logical));
    }
    assert!(index.row(6, 1).is_none());
    let results = GraphIndex::new(order, &commits, true);
    assert_eq!(results.len(), 4);
    assert_eq!(results.row(1, 1).unwrap().logical(), Some(1));
    assert_eq!(results.row(2, 1).unwrap().logical(), Some(2));
}

#[test]
fn million_commit_navigation_reuses_shared_indexes_and_renders_only_the_viewport() {
    let dir = tempfile::tempdir().unwrap();
    let output = Command::new("git")
        .args(["init", "-qb", "main"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let config = Config::load(Some(dir.path().join("config")), &[]).unwrap();
    let mut app = App::new(
        Repository::discover(dir.path()).unwrap(),
        config,
        State::default(),
    );
    app.workers.refresh.cancel();
    app.workers.history.cancel();
    const COUNT: usize = 1_000_000;
    let mut commits = Vec::with_capacity(COUNT);
    commits.resize_with(COUNT, Commit::default);
    commits[500_000].graph_before = vec!["|\\".into(), "|/".into()];
    commits[COUNT - 1].graph_before = vec!["|/".into()];
    commits[COUNT - 1].oid = "active-head".into();
    commits[COUNT - 1].short = "active".into();
    commits[COUNT - 1].subject = "Tail commit".into();
    app.snapshot = Arc::new(Snapshot {
        commits: Arc::new(commits),
        head: "active-head".into(),
        ..Snapshot::default()
    });
    app.loading = false;
    app.history_loading = false;
    app.graph_working_tree = false;
    let started = Instant::now();
    let order = app.filtered_commits();
    let layout = app.graph_index();
    let indexing_ms = started.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(order.len(), COUNT);
    assert_eq!(layout.len(), COUNT + 4);
    assert_eq!(app.working_tree_graph_index(), COUNT - 1);
    let started = Instant::now();
    for selection in (0..COUNT).step_by(997) {
        app.selections[3] = selection;
        assert!(order.ptr_eq(&app.filtered_commits()));
        assert!(Arc::ptr_eq(&layout, &app.graph_index()));
        assert_eq!(app.working_tree_graph_index(), COUNT - 1);
        assert!(std::ptr::eq(
            app.selected_commit().unwrap(),
            &app.snapshot.commits[selection]
        ));
    }
    let lookup_ms = started.elapsed().as_secs_f64() * 1000.0;
    let mut terminal = Terminal::new(TestBackend::new(160, 42)).unwrap();
    let started = Instant::now();
    for selection in [0, 499_999, 500_000, 500_001, COUNT - 1] {
        app.selections[3] = selection;
        terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
        assert!(Arc::ptr_eq(&layout, &app.graph_index()));
        let graph = ui::areas(Rect::new(0, 0, 160, 42), &app).graph;
        let logical = app.selected_graph_row();
        let visual = layout
            .visual_for_logical(logical, app.working_tree_graph_index())
            .unwrap();
        let row = visual - app.scrolls[3];
        assert_eq!(
            ui::hit_test(
                Rect::new(0, 0, 160, 42),
                &app,
                graph.x + 2,
                graph.y + 1 + row as u16
            ),
            Some((Panel::Commits, logical))
        );
    }
    let render_ms = started.elapsed().as_secs_f64() * 1000.0;
    eprintln!(
        "million commits: index {indexing_ms:.2}ms, 1004 cached lookups {lookup_ms:.2}ms, five viewport renders {render_ms:.2}ms"
    );
    let old_order = app.filtered_commits();
    Arc::make_mut(&mut app.snapshot).head = String::new();
    assert_eq!(app.working_tree_graph_index(), 0);
    assert!(old_order.ptr_eq(&app.filtered_commits()));
    assert!(
        Arc::ptr_eq(&layout, &app.graph_index()),
        "moving HEAD only changes the virtual-row insertion point"
    );
}
