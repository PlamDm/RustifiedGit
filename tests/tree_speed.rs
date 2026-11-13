use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use rustifiedgit::{
    app::App,
    config::{Config, State},
    git::Repository,
    model::{FileEntry, Panel, Snapshot},
    tree,
};
use std::{collections::HashSet, path::Path, process::Command, sync::Arc};

fn file(path: &str, status: u8) -> FileEntry {
    FileEntry {
        path: path.into(),
        old_path: None,
        index: status,
        worktree: b' ',
    }
}

#[test]
fn collapsed_trees_preserve_directory_flags_statuses_order_and_search_results() {
    let files = vec![
        file("parent/hidden/changed.txt", b'M'),
        file("parent/hidden/ignored.log", b'!'),
        file("parent/visible.txt", b' '),
        file("directory-file", b'D'),
        file("directory-file/added.txt", b'A'),
        file("z.txt", b' '),
    ];
    for show_unchanged in [false, true] {
        for changed_only in [false, true] {
            for query in ["", "changed", "ignored", "!ignored"] {
                let expanded = tree::rows(
                    &files,
                    true,
                    show_unchanged,
                    &HashSet::new(),
                    query,
                    changed_only,
                );
                for paths in [
                    vec!["parent"],
                    vec!["parent/hidden"],
                    vec!["directory-file"],
                    vec!["parent", "directory-file"],
                ] {
                    let collapsed: HashSet<_> =
                        paths.iter().map(std::path::PathBuf::from).collect();
                    let actual = tree::rows(
                        &files,
                        true,
                        show_unchanged,
                        &collapsed,
                        query,
                        changed_only,
                    );
                    let expected: Vec<_> = expanded
                        .iter()
                        .filter(|row| {
                            !query.is_empty()
                                || !paths.iter().any(|path| {
                                    row.path != Path::new(path) && row.path.starts_with(path)
                                })
                        })
                        .collect();
                    let describe = |row: &tree::Row| format!("{row:?}");
                    assert_eq!(
                        actual.iter().map(describe).collect::<Vec<_>>(),
                        expected.into_iter().map(describe).collect::<Vec<_>>()
                    );
                }
            }
        }
    }
}

#[test]
fn cached_folder_checkboxes_follow_deselection_filters_and_collapsing() {
    let directory = tempfile::tempdir().unwrap();
    assert!(
        Command::new("git")
            .args(["init", "-qb", "main"])
            .current_dir(directory.path())
            .status()
            .unwrap()
            .success()
    );
    let config = Config::load(Some(directory.path().join("config")), &[]).unwrap();
    let mut app = App::new(
        Repository::discover(directory.path()).unwrap(),
        config,
        State::default(),
    );
    app.workers.refresh.cancel();
    app.snapshot = Arc::new(Snapshot {
        files: vec![
            file("parent/a", b'M'),
            file("parent/b", b'M'),
            file("other", b'M'),
        ],
        ..Snapshot::default()
    });
    app.rebuild();
    app.set_focus(Panel::Files);
    let press = |app: &mut App, code, modifiers| app.handle_key(KeyEvent::new(code, modifiers));
    let checkbox = |app: &App| {
        let row = app
            .rows
            .iter()
            .find(|row| row.path == Path::new("parent"))
            .unwrap();
        app.file_checkbox(row)
    };
    press(&mut app, KeyCode::Char('a'), KeyModifiers::CONTROL);
    assert_eq!(checkbox(&app), "[x] ");
    let child = app
        .rows
        .iter()
        .position(|row| row.path == Path::new("parent/a"))
        .unwrap();
    app.select_list_row(Panel::Files, child);
    press(&mut app, KeyCode::Char('u'), KeyModifiers::NONE);
    assert_eq!(checkbox(&app), "[-] ");
    press(&mut app, KeyCode::Char('f'), KeyModifiers::CONTROL);
    app.paste("^parent/a$".into());
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(checkbox(&app), "[ ] ");
    press(&mut app, KeyCode::Char('a'), KeyModifiers::CONTROL);
    assert_eq!(checkbox(&app), "[x] ");
    press(&mut app, KeyCode::Char('f'), KeyModifiers::CONTROL);
    press(&mut app, KeyCode::Char('u'), KeyModifiers::CONTROL);
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(checkbox(&app), "[-] ");
    press(&mut app, KeyCode::Char('a'), KeyModifiers::CONTROL);
    app.collapsed.insert("parent".into());
    app.rebuild();
    assert_eq!(checkbox(&app), "[x] ");
    let parent = app
        .rows
        .iter()
        .position(|row| row.path == Path::new("parent"))
        .unwrap();
    app.select_list_row(Panel::Files, parent);
    press(&mut app, KeyCode::Char('u'), KeyModifiers::NONE);
    assert_eq!(checkbox(&app), "[ ] ");
    assert_eq!(app.file_selected_count(), 1);

    // The working-tree graph row also forwards file marking commands.
    app.collapsed.clear();
    app.rebuild();
    let child = app
        .rows
        .iter()
        .position(|row| row.path == Path::new("parent/a"))
        .unwrap();
    app.select_list_row(Panel::Files, child);
    app.set_focus(Panel::Commits);
    assert_eq!(checkbox(&app), "[ ] ");
    press(&mut app, KeyCode::Char('v'), KeyModifiers::NONE);
    assert_eq!(checkbox(&app), "[-] ");
    press(&mut app, KeyCode::Char('v'), KeyModifiers::NONE);
    assert_eq!(checkbox(&app), "[ ] ");
}
