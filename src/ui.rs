use crate::{
    app::{App, Modal, panel_index, strip_ansi},
    model::{Panel, RefKind},
    tree::Row,
};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};

const ACCENT: Color = Color::Cyan;
const MUTED: Color = Color::DarkGray;
const SELECTED: Color = Color::Rgb(35, 55, 65);

pub struct Areas {
    pub header: Rect,
    pub footer: Rect,
    pub left: Vec<Rect>,
    pub diff: Rect,
    pub output: Rect,
}

pub fn areas(area: Rect, app: &App) -> Areas {
    let vertical = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(2),
    ])
    .split(area);
    if app.fullscreen {
        return Areas {
            header: vertical[0],
            footer: vertical[2],
            left: vec![Rect::default(); 5],
            diff: if app.focus == Panel::Output {
                Rect::default()
            } else {
                vertical[1]
            },
            output: if app.focus == Panel::Output {
                vertical[1]
            } else {
                Rect::default()
            },
        };
    }
    let columns = Layout::horizontal([
        Constraint::Percentage((app.config.gui.side_panel_width * 100.0) as u16),
        Constraint::Min(10),
    ])
    .split(vertical[1]);
    let focused = match app.focus {
        Panel::Files | Panel::CommitFiles => 1,
        Panel::Branches => 2,
        Panel::Commits => 3,
        Panel::Stash => 4,
        _ => 1,
    };
    let rest = columns[0].height.saturating_sub(3);
    let small = if rest >= 30 { 5 } else { 3 };
    let mut constraints = vec![Constraint::Length(3)];
    for panel in 1..5 {
        constraints.push(if panel == focused {
            Constraint::Min(4)
        } else {
            Constraint::Length(small)
        });
    }
    let left = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(columns[0])
        .to_vec();
    let right = Layout::vertical(if app.state.hide_command_log {
        vec![Constraint::Min(1), Constraint::Length(0)]
    } else {
        vec![
            Constraint::Min(4),
            Constraint::Percentage(if app.focus == Panel::Output { 55 } else { 25 }),
        ]
    })
    .split(columns[1]);
    Areas {
        header: vertical[0],
        footer: vertical[2],
        left,
        diff: right[0],
        output: right[1],
    }
}

fn block(title: String, focused: bool) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            format!(" {title} "),
            Style::default()
                .fg(if focused { ACCENT } else { Color::Gray })
                .add_modifier(Modifier::BOLD),
        ))
        .border_style(Style::default().fg(if focused { ACCENT } else { MUTED }))
}

fn clean(text: &str) -> String {
    strip_ansi(text).replace('\r', "\\r").replace('\n', "\\n")
}

fn draw_list(
    frame: &mut Frame,
    area: Rect,
    title: String,
    panel: Panel,
    app: &mut App,
    items: Vec<Line<'static>>,
    total: usize,
) {
    if area.width < 2 || area.height < 2 {
        return;
    }
    let selected = app.selected(panel).min(total.saturating_sub(1));
    let index = panel_index(panel);
    let mut state = ListState::default().with_selected((total > 0).then_some(selected));
    *state.offset_mut() = app.scrolls[index];
    let widget = List::new(items)
        .block(block(title, app.focus == panel))
        .highlight_style(Style::default().bg(SELECTED))
        .highlight_symbol("› ");
    frame.render_stateful_widget(widget, area, &mut state);
    app.scrolls[index] = state.offset();
}

fn file_line(row: &Row, app: &App) -> Line<'static> {
    let color = if row.ignored || !row.changed {
        MUTED
    } else if row.status.contains('U') {
        Color::Magenta
    } else if row.status.starts_with('?') {
        Color::Red
    } else {
        Color::White
    };
    let index = row.status.chars().next().unwrap_or(' ');
    let worktree = row.status.chars().nth(1).unwrap_or(' ');
    let marker = if app.marked.contains(&row.path) {
        "●"
    } else if row.directory {
        if app.collapsed.contains(&row.path) {
            "▸"
        } else {
            "▾"
        }
    } else {
        " "
    };
    Line::from(vec![
        Span::styled(
            index.to_string(),
            Style::default().fg(if index != ' ' && index != '?' && index != '!' {
                Color::Green
            } else {
                color
            }),
        ),
        Span::styled(
            worktree.to_string(),
            Style::default().fg(if worktree != ' ' && worktree != '!' {
                Color::Red
            } else {
                color
            }),
        ),
        Span::raw(format!(" {}{marker} ", "  ".repeat(row.depth))),
        Span::styled(clean(&row.label), Style::default().fg(color)),
    ])
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    if area.width < 36 || area.height < 12 {
        frame.render_widget(
            Paragraph::new("Enlarge the terminal to at least 36 × 12.\nPress q to quit."),
            area,
        );
        return;
    }
    let areas = areas(area, app);
    let changed = app
        .snapshot
        .files
        .iter()
        .filter(|file| file.changed())
        .count();
    let staged = app
        .snapshot
        .files
        .iter()
        .filter(|file| file.staged())
        .count();
    let name = app
        .repo
        .root
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    let activity = if let Some((_, started, label)) = &app.busy {
        format!(
            " {} {}s · {}",
            ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"]
                [started.elapsed().as_millis() as usize / 100 % 8],
            started.elapsed().as_secs(),
            clean(label)
        )
    } else if app.loading {
        " refreshing…".into()
    } else {
        format!(
            " {} changes · {} staged · {}ms refresh",
            changed, staged, app.snapshot.elapsed_ms
        )
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled(
                    " PD LAZYGIT ",
                    Style::default()
                        .bg(ACCENT)
                        .fg(Color::Black)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("  {}", clean(&name)),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(activity, Style::default().fg(MUTED)),
            ]),
            Line::styled(
                format!(" {}", clean(&app.repo.root.to_string_lossy())),
                Style::default().fg(MUTED),
            ),
        ]),
        areas.header,
    );
    if !app.fullscreen {
        let status = format!(
            "{}{}",
            app.snapshot.branch,
            if app.snapshot.operation.is_empty() {
                String::new()
            } else {
                format!(" · {}", app.snapshot.operation)
            }
        );
        frame.render_widget(
            Paragraph::new(clean(&status))
                .block(block("1 Status".into(), app.focus == Panel::Status)),
            areas.left[0],
        );
        let commit_files = app.side == Panel::CommitFiles;
        let panel = if commit_files {
            Panel::CommitFiles
        } else {
            Panel::Files
        };
        let rows = if commit_files {
            &app.commit_rows
        } else {
            &app.rows
        };
        let title = if commit_files {
            format!(
                "2 Commit files {}",
                &app.commit_oid[..app.commit_oid.len().min(8)]
            )
        } else {
            format!(
                "2 Files{}{}",
                if app.state.show_ignored {
                    " +ignored"
                } else {
                    ""
                },
                if app.changed_only { " [changed]" } else { "" }
            )
        };
        let total = rows.len();
        let height = areas.left[1].height.saturating_sub(2).max(1) as usize;
        let index = panel_index(panel);
        let selected = app.selected(panel).min(total.saturating_sub(1));
        let mut offset = app.scrolls[index].min(total.saturating_sub(height));
        if selected < offset {
            offset = selected;
        }
        if selected >= offset + height {
            offset = selected + 1 - height;
        }
        let lines: Vec<_> = rows
            .iter()
            .skip(offset)
            .take(height)
            .map(|row| ListItem::new(file_line(row, app)))
            .collect();
        let mut state =
            ListState::default().with_selected((total > 0).then_some(selected - offset));
        frame.render_stateful_widget(
            List::new(lines)
                .block(block(title, app.focus == panel))
                .highlight_style(Style::default().bg(SELECTED))
                .highlight_symbol("› "),
            areas.left[1],
            &mut state,
        );
        app.scrolls[index] = offset;
        let tabs = ["Branches", "Remotes", "Tags", "Worktrees"];
        let lines: Vec<_> = if app.branch_tab == 3 {
            app.snapshot
                .worktrees
                .iter()
                .map(|worktree| {
                    Line::from(vec![
                        Span::styled(
                            format!("{} ", worktree.branch),
                            Style::default().fg(Color::Green),
                        ),
                        Span::raw(clean(&worktree.path.to_string_lossy())),
                    ])
                })
                .collect()
        } else {
            app.filtered_refs()
                .iter()
                .map(|index| {
                    let reference = &app.snapshot.references[*index];
                    Line::from(vec![
                        Span::styled(
                            if reference.current { "* " } else { "  " },
                            Style::default().fg(Color::Green),
                        ),
                        Span::styled(
                            clean(&reference.name),
                            Style::default().fg(match reference.kind {
                                RefKind::Branch => Color::Green,
                                RefKind::Remote => Color::Yellow,
                                RefKind::Tag => Color::Magenta,
                            }),
                        ),
                    ])
                })
                .collect()
        };
        let total = lines.len();
        draw_list(
            frame,
            areas.left[2],
            format!("3 {} [ ]", tabs[app.branch_tab]),
            Panel::Branches,
            app,
            lines,
            total,
        );
        let lines: Vec<_> = app
            .filtered_commits()
            .iter()
            .map(|index| {
                let commit = &app.snapshot.commits[*index];
                Line::from(vec![
                    Span::styled(
                        format!("{} ", commit.short),
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::raw(clean(&commit.subject)),
                ])
            })
            .collect();
        let total = lines.len();
        draw_list(
            frame,
            areas.left[3],
            format!(
                "4 {}{}",
                if app.commit_tab == 1 {
                    "Reflog"
                } else {
                    "Commits"
                },
                if app.options.history_path.is_some() {
                    " [path]"
                } else {
                    ""
                }
            ),
            Panel::Commits,
            app,
            lines,
            total,
        );
        let lines: Vec<_> = app
            .filtered_stashes()
            .iter()
            .map(|index| {
                let stash = &app.snapshot.stashes[*index];
                Line::raw(format!("{} {}", stash.reference, clean(&stash.subject)))
            })
            .collect();
        let total = lines.len();
        draw_list(
            frame,
            areas.left[4],
            "5 Stash".into(),
            Panel::Stash,
            app,
            lines,
            total,
        );
    }
    draw_diff(frame, areas.diff, app);
    draw_output(frame, areas.output, app);
    let context_help = match app.focus {
        Panel::Files => {
            "space stage/unstage · c commit · a stage all · I ignored · F history · Ctrl+t difftool"
        }
        Panel::CommitFiles => "Enter diff · F history · c restore · Ctrl+t difftool · Esc commits",
        Panel::Branches => "space checkout · n new · M merge · r rebase · [ ] tabs · w worktree",
        Panel::Commits => {
            "Enter files · g reset · t revert · C copy · V cherry-pick · i rebase · . load more"
        }
        Panel::Stash => "space apply · g pop · d drop · s new stash",
        Panel::Diff => {
            "space apply · d discard · ←/→ hunks · a hunk/line · [ ] staged · Esc return"
        }
        Panel::Output => "j/k scroll · End follow output · Ctrl+c cancel command",
        Panel::Status => "Enter repository actions · Ctrl+r recent repositories",
    };
    let filter = app.search(app.focus);
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(
                format!(" {}", clean(&app.notice)),
                Style::default().fg(if app.busy.is_some() {
                    Color::Yellow
                } else {
                    MUTED
                }),
            ),
            Line::styled(
                if filter.is_empty() {
                    format!(" {context_help} · ? help")
                } else {
                    format!(" / {filter} · Esc clears filter · {context_help}")
                },
                Style::default().fg(ACCENT),
            ),
        ]),
        areas.footer,
    );
    draw_modal(frame, app);
}

fn draw_diff(frame: &mut Frame, area: Rect, app: &mut App) {
    if area.height < 2 || area.width < 2 {
        return;
    }
    let height = area.height.saturating_sub(2) as usize;
    if app.focus == Panel::Diff {
        if app.diff_line < app.diff_scroll {
            app.diff_scroll = app.diff_line;
        }
        if app.diff_line >= app.diff_scroll + height {
            app.diff_scroll = app.diff_line.saturating_sub(height.saturating_sub(1));
        }
    }
    let selected_hunk = app.patch.hunk_at(app.diff_line);
    let lines: Vec<_> = app
        .preview_lines
        .iter()
        .enumerate()
        .skip(app.diff_scroll)
        .take(height)
        .map(|(index, line)| {
            let color = if line.starts_with("+++")
                || line.starts_with("---")
                || line.starts_with("diff ")
            {
                Color::Yellow
            } else if line.starts_with('+') {
                Color::Green
            } else if line.starts_with('-') {
                Color::Red
            } else if line.starts_with("@@") {
                Color::Cyan
            } else {
                Color::Gray
            };
            let selected = app.focus == Panel::Diff
                && if app.hunk_mode {
                    selected_hunk.is_some_and(|hunk| app.patch.hunks[hunk].lines.contains(&index))
                } else {
                    index == app.diff_line
                };
            Line::styled(
                clean(line)
                    .chars()
                    .skip(app.horizontal_scroll)
                    .collect::<String>(),
                Style::default()
                    .fg(color)
                    .bg(if selected { SELECTED } else { Color::Reset }),
            )
        })
        .collect();
    let title = format!(
        "0 {}{}{}",
        if app.preview.title.is_empty() {
            "Preview"
        } else {
            &app.preview.title
        },
        if app.preview.staged { " [staged]" } else { "" },
        if app.preview_loading { " …" } else { "" }
    );
    frame.render_widget(
        Paragraph::new(lines).block(block(title, app.focus == Panel::Diff)),
        area,
    );
}

fn draw_output(frame: &mut Frame, area: Rect, app: &mut App) {
    if area.height < 2 || area.width < 2 {
        return;
    }
    let height = area.height.saturating_sub(2) as usize;
    if app.output_follow {
        app.output_scroll =
            (app.output.len() + usize::from(!app.output_tail().is_empty())).saturating_sub(height);
    }
    let lines: Vec<_> = app
        .output
        .iter()
        .map(String::as_str)
        .chain(std::iter::once(app.output_tail()).filter(|line| !line.is_empty()))
        .skip(app.output_scroll)
        .take(height)
        .map(|line| {
            Line::styled(
                clean(line),
                Style::default().fg(if line.starts_with("$ ") {
                    Color::Yellow
                } else if line.starts_with('✗') {
                    Color::Red
                } else {
                    Color::Gray
                }),
            )
        })
        .collect();
    frame.render_widget(
        Paragraph::new(lines).block(block("6 Command output".into(), app.focus == Panel::Output)),
        area,
    );
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(2));
    let height = height.min(area.height.saturating_sub(2));
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

fn draw_modal(frame: &mut Frame, app: &App) {
    let Some(modal) = &app.modal else {
        return;
    };
    let area = frame.area();
    match modal {
        Modal::Input { title, text, .. } => {
            let rect = centered(area, 88, 7);
            frame.render_widget(Clear, rect);
            frame.render_widget(
                Paragraph::new(format!(
                    "{text}\n\nEnter submit · Esc cancel · Ctrl+u clear"
                ))
                .wrap(Wrap { trim: false })
                .block(block(title.clone(), true)),
                rect,
            );
            let width = rect.width.saturating_sub(2).max(1) as usize;
            let cursor = Line::from(text.as_str()).width();
            frame.set_cursor_position((
                rect.x + 1 + (cursor % width) as u16,
                (rect.y + 1 + (cursor / width) as u16).min(rect.bottom().saturating_sub(2)),
            ));
        }
        Modal::Confirm { title, .. } => {
            let rect = centered(area, 88, 9);
            frame.render_widget(Clear, rect);
            frame.render_widget(Paragraph::new(format!("{title}\n\nThis operation changes the repository.\n\ny / Enter confirm · n / Esc cancel")).wrap(Wrap { trim: false }).block(block("Confirm".into(), true)), rect);
        }
        Modal::Menu {
            title,
            items,
            selected,
        } => {
            let rect = centered(area, 96, (items.len() + 4).min(25) as u16);
            frame.render_widget(Clear, rect);
            let lines: Vec<_> = items
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    ListItem::new(format!(
                        "{} {}",
                        if index < 9 {
                            format!("{}.", index + 1)
                        } else {
                            "  ".into()
                        },
                        item.label
                    ))
                })
                .collect();
            let mut state = ListState::default().with_selected(Some(*selected));
            frame.render_stateful_widget(
                List::new(lines)
                    .block(block(title.clone(), true))
                    .highlight_style(Style::default().bg(SELECTED))
                    .highlight_symbol("› "),
                rect,
                &mut state,
            );
        }
        Modal::Help { scroll } => {
            let rect = centered(area, 105, area.height.saturating_sub(4));
            frame.render_widget(Clear, rect);
            frame.render_widget(
                Paragraph::new(HELP)
                    .scroll(((*scroll).min(u16::MAX as usize) as u16, 0))
                    .block(block("Keys · j/k scroll · Esc close".into(), true)),
                rect,
            );
        }
        Modal::Message(message) => {
            let rect = centered(area, 96, 12);
            frame.render_widget(Clear, rect);
            frame.render_widget(
                Paragraph::new(format!("{message}\n\nEnter / Esc close"))
                    .wrap(Wrap { trim: false })
                    .block(block("Message".into(), true)),
                rect,
            );
        }
    }
}

pub fn hit_test(area: Rect, app: &App, x: u16, y: u16) -> Option<(Panel, usize)> {
    let areas = areas(area, app);
    for (index, rect) in areas.left.iter().enumerate() {
        if rect.contains((x, y).into()) {
            let panel = [
                Panel::Status,
                if app.side == Panel::CommitFiles {
                    Panel::CommitFiles
                } else {
                    Panel::Files
                },
                Panel::Branches,
                Panel::Commits,
                Panel::Stash,
            ][index];
            return Some((
                panel,
                app.scrolls[panel_index(panel)] + y.saturating_sub(rect.y + 1) as usize,
            ));
        }
    }
    if areas.diff.contains((x, y).into()) {
        return Some((
            Panel::Diff,
            app.diff_scroll + y.saturating_sub(areas.diff.y + 1) as usize,
        ));
    }
    if areas.output.contains((x, y).into()) {
        return Some((
            Panel::Output,
            app.output_scroll + y.saturating_sub(areas.output.y + 1) as usize,
        ));
    }
    None
}

pub const HELP: &str = "Navigation\n  1 status  2 files  3 branches  4 commits  5 stash  6 command output\n  j/k or arrows move · Tab / Shift+Tab change panel · 0 focus diff\n  / search/filter · Esc return/clear filter · + fullscreen · q quit\n  Ctrl+r recent repos · Ctrl+s filters · @ show/hide output\n\nFiles and your fork's browsing\n  Unchanged tracked files are dimmed · I toggle ignored files\n  ` tree/flat · - collapse all · = expand all · Enter expand/open diff\n  space stage/unstage selection · a stage/unstage all · v mark files\n  Shift+up/down mark a range · d discard selection · D discard/reset menu\n  c commit · C commit in editor · A amend · w commit without hooks\n  s stash with untracked files · S stash options · i add to .gitignore\n  F file/directory history (follows file renames) · Ctrl+s clear history\n  e editor · o open file/submodule · y copy path · r refresh · f fetch\n  Ctrl+t configured Git difftool · Alt+m configured Git mergetool\n  M conflict resolution (ours/theirs/tool) · space marks resolved\n\nDiffs\n  ~ persist automatic preview on/off · 0/Enter explicitly bypass limits\n  Automatic file versions: 2 MiB · automatic output: 512 KiB\n  Left/right select hunk · j/k select line · a toggle hunk/line mode\n  space stage/unstage hunk or line · d discard selected change\n  [ / ] switch staged/unstaged · { / } change context · Ctrl+w whitespace\n  H/L horizontal scroll · n/N next/previous file · Esc return\n\nBranches, remotes, tags, worktrees\n  [ / ] change tab · space checkout · n new branch · c checkout by name\n  R rename · M merge into current · r rebase onto selected · d delete\n  F force checkout (confirmed) · T new tag · u set upstream · P push\n  w create worktree · Enter on worktree opens it\n\nCommits\n  [ / ] history/reflog · Enter commit files · space detached checkout\n  g reset soft/mixed/hard (confirmed) · t revert · T tag\n  C copy commit · V cherry-pick copied commits · F create fixup\n  i interactive rebase in editor · r/R reword HEAD or open rebase editor\n  y copy hash · b bisect controls · . load 300 more commits\n\nStashes and running operations\n  space apply · g pop · d drop · Enter diff · s save new stash\n  m rebase/merge/cherry-pick/revert continue/abort/skip menu\n  p pull · P push · R refresh · Ctrl+c cancel active command\n  : run command with live output · ! run interactive command\n  Panel 6 keeps command output available while the command runs\n  End follows output · Home/j/k/PageUp/PageDown scroll\n\nConfiguration\n  --use-config-dir DIR and --use-config-file FILE accept LazyGit YAML\n  Fork preview settings, file visibility, refresh, common key remaps,\n  editor/open commands, and selected-value custom commands are supported.\n  See PORTING.md for the exact compatibility boundary.\n";
