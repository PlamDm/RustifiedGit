use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Terminal,
    backend::TestBackend,
    style::{Color, Modifier},
};
use rustifiedgit::{
    app::App,
    config::{Config, State},
    fuzzy::Query,
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
    for (key, value) in [
        ("user.name", "Highlight Tester"),
        ("user.email", "highlight@example.invalid"),
        ("commit.gpgsign", "false"),
        ("core.hooksPath", ".test-hooks"),
    ] {
        git(&root, &["config", key, value]);
    }
    fs::create_dir(root.join("src")).unwrap();
    for name in ["config.rs", "Пример.rs"] {
        fs::write(root.join("src").join(name), "original\n").unwrap();
    }
    git(&root, &["add", "."]);
    let subject = format!("Промяна cfg 界 {}", "long subject ".repeat(30));
    let output = Command::new("git")
        .args(["commit", "-qm", &subject])
        .current_dir(&root)
        .env("GIT_AUTHOR_DATE", "2026-11-26T12:00:00Z")
        .env("GIT_COMMITTER_DATE", "2026-11-26T12:00:00Z")
        .output()
        .unwrap();
    assert!(output.status.success());
    for name in ["config.rs", "Пример.rs"] {
        fs::write(root.join("src").join(name), "changed\n").unwrap();
    }
    let config = Config::load(Some(dir.path().join("config")), &[]).unwrap();
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
        assert!(Instant::now() < deadline, "{}", app.notice);
        std::thread::sleep(Duration::from_millis(5));
    }
    (dir, app)
}

fn search(app: &mut App, panel: Panel, query: &str, accept: bool) {
    app.set_focus(panel);
    app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    app.paste(query.to_owned());
    if accept {
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    }
}

fn screen(app: &mut App, width: u16) -> Terminal<TestBackend> {
    let mut terminal = Terminal::new(TestBackend::new(width, 42)).unwrap();
    terminal.draw(|frame| ui::draw(frame, app)).unwrap();
    terminal
}

fn find(terminal: &Terminal<TestBackend>, area: ratatui::layout::Rect, needle: &str) -> (u16, u16) {
    let buffer = terminal.backend().buffer();
    for y in area.y..area.bottom() {
        let row: String = (area.x..area.right())
            .map(|x| buffer[(x, y)].symbol())
            .collect();
        if let Some(start) = row.find(needle) {
            return (area.x + row[..start].chars().count() as u16, y);
        }
    }
    panic!("missing {needle}");
}

fn highlighted(terminal: &Terminal<TestBackend>, x: u16, y: u16) -> bool {
    let cell = &terminal.backend().buffer()[(x, y)];
    cell.fg == Color::Yellow
        && cell
            .modifier
            .contains(Modifier::BOLD | Modifier::UNDERLINED)
}

#[test]
fn positions_follow_fuzzy_ranking_literals_anchors_exclusions_and_unicode_case() {
    let text = "caaaafaaaag cfg";
    let start = text.find("cfg").unwrap();
    assert_eq!(
        Query::new("cfg").positions(text),
        Some((start..start + 3).collect())
    );
    assert_eq!(
        Query::new("'config").positions("src/config.rs"),
        Some((4..10).collect())
    );
    assert_eq!(
        Query::new("^src .rs$").positions("src/config.rs"),
        Some(vec![0, 1, 2, 10, 11, 12])
    );
    assert_eq!(Query::new("!test").positions("src/config.rs"), Some(vec![]));
    assert_eq!(Query::new("cfg !test").positions("test/config.rs"), None);
    assert_eq!(Query::new("прм").positions("Промяна"), Some(vec![0, 1, 3]));
    assert_eq!(Query::new("i\u{307}").positions("İ"), Some(vec![0]));
    assert_eq!(Query::new("ος").positions("ΟΣ"), Some(vec![0, 1]));
    assert!(Query::new("ος").score("ΟΣ").is_some());
    assert_eq!(Query::new("Foo").positions("foo"), None);
    assert_eq!(Query::new("").positions("anything"), Some(vec![]));
}

#[test]
fn graph_highlights_only_matches_while_typing_and_after_accepting_with_metadata_aligned() {
    let (_dir, mut app) = fixture();
    for accept in [false, true] {
        if !accept {
            search(&mut app, Panel::Commits, "прм cfg", false);
        } else {
            app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        }
        for width in [140, 180] {
            let terminal = screen(&mut app, width);
            let graph = ui::areas(ratatui::layout::Rect::new(0, 0, width, 42), &app).graph;
            let (x, y) = find(&terminal, graph, "Промяна");
            assert!(highlighted(&terminal, x, y));
            assert!(highlighted(&terminal, x + 1, y));
            assert!(!highlighted(&terminal, x + 2, y));
            assert!(highlighted(&terminal, x + 3, y));
            assert!(!highlighted(&terminal, x + 4, y));
            let (cfg_x, cfg_y) = find(&terminal, graph, "cfg");
            assert!((0..3).all(|offset| highlighted(&terminal, cfg_x + offset, cfg_y)));
            let (date_x, date_y) = find(&terminal, graph, "26 Nov 2026");
            assert_eq!(date_y, y);
            assert_eq!(date_x + 11, graph.right() - 1);
            assert!(!highlighted(&terminal, date_x, date_y));
            for column in graph.x + 1..graph.right() - 1 {
                if terminal.backend().buffer()[(column, y)].symbol() == "…" {
                    assert!(!highlighted(&terminal, column, y));
                }
            }
        }
    }
}

#[test]
fn file_highlights_map_query_positions_across_directory_and_basename_without_style_spill() {
    let (_dir, mut app) = fixture();
    app.collapsed.insert("src".into());
    search(&mut app, Panel::Files, "src cfg", true);
    let terminal = screen(&mut app, 160);
    let files = ui::areas(ratatui::layout::Rect::new(0, 0, 160, 42), &app).files;
    let (src_x, src_y) = find(&terminal, files, "src");
    assert!((0..3).all(|offset| highlighted(&terminal, src_x + offset, src_y)));
    let (x, y) = find(&terminal, files, "config.rs");
    for offset in [0, 3, 5] {
        assert!(highlighted(&terminal, x + offset, y), "offset {offset}");
    }
    for offset in [1, 2, 4, 6, 7, 8] {
        assert!(!highlighted(&terminal, x + offset, y), "offset {offset}");
    }
    assert!(!highlighted(&terminal, x - 1, y));
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    search(&mut app, Panel::Files, "src пр", true);
    let terminal = screen(&mut app, 160);
    let (x, y) = find(&terminal, files, "Пример.rs");
    assert!(highlighted(&terminal, x, y));
    assert!(highlighted(&terminal, x + 1, y));
    assert!(!highlighted(&terminal, x + 2, y));
}

#[test]
fn graph_hash_and_active_branch_decorations_keep_their_styles_and_highlight_matches() {
    let (_dir, mut app) = fixture();
    let hash = app.snapshot.commits[0].short.clone();
    search(&mut app, Panel::Commits, &hash, true);
    let terminal = screen(&mut app, 160);
    let graph = ui::areas(ratatui::layout::Rect::new(0, 0, 160, 42), &app).graph;
    let (x, y) = find(&terminal, graph, &hash);
    assert!(highlighted(&terminal, x, y));
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    search(&mut app, Panel::Commits, "'main", true);
    let terminal = screen(&mut app, 160);
    let (x, y) = find(&terminal, graph, "main");
    assert!((0..4).all(|offset| highlighted(&terminal, x + offset, y)));
    assert!(!highlighted(&terminal, x - 1, y));
}

#[test]
fn ctrl_f_branch_search_filters_and_highlights_nested_branch_names() {
    let (_dir, mut app) = fixture();
    git(&app.repo.root, &["branch", "feature/configuration"]);
    app.refresh();
    let deadline = Instant::now() + Duration::from_secs(8);
    while app.loading {
        app.poll_workers();
        assert!(Instant::now() < deadline, "{}", app.notice);
        std::thread::sleep(Duration::from_millis(5));
    }
    app.set_focus(Panel::Branches);
    app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL));
    app.paste("feat".into());
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.refs_for(false).len(), 1);
    let terminal = screen(&mut app, 180);
    let branches = ui::areas(ratatui::layout::Rect::new(0, 0, 180, 42), &app).local;
    let (x, y) = find(&terminal, branches, "feature");
    assert!(highlighted(&terminal, x, y));
    assert!(highlighted(&terminal, x + 1, y));
    assert!(highlighted(&terminal, x + 2, y));
    assert!(highlighted(&terminal, x + 3, y));
}
