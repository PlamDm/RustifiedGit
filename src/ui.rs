use crate::{
    app::{App, Modal, panel_index, strip_ansi},
    model::Panel,
    tree::Row,
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
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
    pub repositories: Rect,
    pub local: Rect,
    pub remote: Rect,
    pub stash: Rect,
    pub graph: Rect,
    pub details: Rect,
    pub files: Rect,
    pub diff: Rect,
    pub output: Rect,
}

impl Areas {
    pub fn pane(&self, panel: Panel) -> Rect {
        match panel {
            Panel::Status => self.repositories,
            Panel::Branches => self.local,
            Panel::RemoteBranches => self.remote,
            Panel::Stash => self.stash,
            Panel::Commits => self.graph,
            Panel::Details => self.details,
            Panel::Files | Panel::CommitFiles => self.files,
            Panel::Diff => self.diff,
            Panel::Output => self.output,
        }
    }
}

pub fn areas(area: Rect, app: &App) -> Areas {
    let vertical = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(2),
    ])
    .split(area);
    let mut result = Areas {
        header: vertical[0],
        footer: vertical[2],
        repositories: Rect::default(),
        local: Rect::default(),
        remote: Rect::default(),
        stash: Rect::default(),
        graph: Rect::default(),
        details: Rect::default(),
        files: Rect::default(),
        diff: Rect::default(),
        output: Rect::default(),
    };
    if app.fullscreen {
        match app.focus {
            Panel::Status => result.repositories = vertical[1],
            Panel::Branches => result.local = vertical[1],
            Panel::RemoteBranches => result.remote = vertical[1],
            Panel::Stash => result.stash = vertical[1],
            Panel::Commits => result.graph = vertical[1],
            Panel::Details => result.details = vertical[1],
            Panel::Files | Panel::CommitFiles => result.files = vertical[1],
            Panel::Diff => result.diff = vertical[1],
            Panel::Output => result.output = vertical[1],
        }
        return result;
    }
    let output_height = if app.state.hide_command_log {
        0
    } else if app.focus == Panel::Output {
        (vertical[1].height / 3).max(5)
    } else {
        5.min(vertical[1].height / 3)
    };
    let body = Layout::vertical([Constraint::Min(1), Constraint::Length(output_height)])
        .split(vertical[1]);
    result.output = body[1];
    let sidebar_width = (area.width as f64 * app.config.gui.side_panel_width)
        .round()
        .clamp(19.0, 32.0) as u16;
    let columns =
        Layout::horizontal([Constraint::Length(sidebar_width), Constraint::Min(1)]).split(body[0]);
    let left = Layout::vertical([
        Constraint::Length(4),
        Constraint::Fill(4),
        Constraint::Fill(3),
        Constraint::Fill(2),
    ])
    .split(columns[0]);
    result.repositories = left[0];
    result.local = left[1];
    result.remote = left[2];
    result.stash = left[3];
    let workspace =
        Layout::vertical([Constraint::Percentage(65), Constraint::Min(4)]).split(columns[1]);
    result.diff = workspace[1];
    let top =
        Layout::horizontal([Constraint::Percentage(58), Constraint::Min(22)]).split(workspace[0]);
    result.graph = top[0];
    let inspector =
        Layout::vertical([Constraint::Percentage(48), Constraint::Min(3)]).split(top[1]);
    result.details = inspector[0];
    result.files = inspector[1];
    result
}

fn block(title: impl Into<String>, focused: bool) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            format!(" {} ", title.into()),
            Style::default()
                .fg(if focused { ACCENT } else { Color::Gray })
                .add_modifier(Modifier::BOLD),
        ))
        .border_style(Style::default().fg(if focused { ACCENT } else { MUTED }))
}

fn clean(text: &str) -> String {
    strip_ansi(text).replace('\r', "\\r").replace('\n', "\\n")
}

fn elide(text: &str, width: usize) -> String {
    if Line::raw(text).width() <= width {
        return text.to_owned();
    }
    let mut result = String::new();
    let mut used = 0;
    for character in text.chars() {
        let mut buffer = [0; 4];
        let character_text: &str = character.encode_utf8(&mut buffer);
        let size = Span::raw(character_text).width();
        if used + size + 1 > width {
            break;
        }
        used += size;
        result.push(character);
    }
    if width > 0 {
        result.push('…');
    }
    result
}

fn viewport(
    app: &mut App,
    panel: Panel,
    total: usize,
    selected: usize,
    height: usize,
) -> std::ops::Range<usize> {
    let height = height.max(1);
    let index = panel_index(panel);
    let selected = selected.min(total.saturating_sub(1));
    let mut offset = app.scrolls[index].min(total.saturating_sub(height));
    if selected < offset {
        offset = selected;
    }
    if selected >= offset + height {
        offset = selected + 1 - height;
    }
    app.scrolls[index] = offset;
    offset..(offset + height).min(total)
}

fn render_list(
    frame: &mut Frame,
    area: Rect,
    title: String,
    focused: bool,
    items: Vec<Line<'static>>,
    selected: Option<usize>,
) {
    if area.width < 2 || area.height < 2 {
        return;
    }
    let mut state = ListState::default().with_selected(selected);
    frame.render_stateful_widget(
        List::new(items)
            .block(block(title, focused))
            .highlight_symbol("› ")
            .highlight_style(Style::default().bg(SELECTED)),
        area,
        &mut state,
    );
}

fn draw_repositories(frame: &mut Frame, area: Rect, app: &mut App) {
    if area.height < 2 {
        return;
    }
    let selected = app.selected(Panel::Status);
    let range = viewport(
        app,
        Panel::Status,
        app.state.recent_repos.len(),
        selected,
        area.height.saturating_sub(2) as usize,
    );
    let items = app.state.recent_repos[range.clone()]
        .iter()
        .map(|path| {
            let active = path == &app.repo.root;
            Line::from(vec![
                Span::styled(
                    if active { "● " } else { "  " },
                    Style::default().fg(if active { ACCENT } else { MUTED }),
                ),
                Span::raw(clean(
                    &path.file_name().unwrap_or_default().to_string_lossy(),
                )),
            ])
        })
        .collect();
    render_list(
        frame,
        area,
        "1 Repositories".into(),
        app.focus == Panel::Status,
        items,
        (!range.is_empty()).then_some(selected.saturating_sub(range.start)),
    );
}

fn draw_refs(frame: &mut Frame, area: Rect, app: &mut App, remote: bool) {
    if area.height < 2 {
        return;
    }
    let panel = if remote {
        Panel::RemoteBranches
    } else {
        Panel::Branches
    };
    let selected = app.selected(panel);
    if !remote && app.branch_tab == 3 {
        let range = viewport(
            app,
            panel,
            app.snapshot.worktrees.len(),
            selected,
            area.height.saturating_sub(2) as usize,
        );
        let items = app.snapshot.worktrees[range.clone()]
            .iter()
            .map(|worktree| {
                Line::raw(format!(
                    "{} {}",
                    worktree.branch,
                    clean(&worktree.path.to_string_lossy())
                ))
            })
            .collect();
        render_list(
            frame,
            area,
            "2 Worktrees [ ]".into(),
            app.focus == panel,
            items,
            (!range.is_empty()).then_some(selected.saturating_sub(range.start)),
        );
        return;
    }
    let indices = app.refs_for(remote);
    let range = viewport(
        app,
        panel,
        indices.len(),
        selected,
        area.height.saturating_sub(2) as usize,
    );
    let items = indices[range.clone()]
        .iter()
        .map(|index| {
            let reference = &app.snapshot.references[*index];
            Line::from(vec![
                Span::styled(
                    if app.branch_visible(reference) {
                        "[x] "
                    } else {
                        "[ ] "
                    },
                    Style::default().fg(if app.branch_visible(reference) {
                        ACCENT
                    } else {
                        MUTED
                    }),
                ),
                Span::styled(
                    clean(&reference.name),
                    Style::default().fg(if remote { Color::Yellow } else { Color::Green }),
                ),
                Span::styled(
                    if reference.current { " *" } else { "" },
                    Style::default().fg(ACCENT),
                ),
            ])
        })
        .collect();
    let title = if remote {
        "3 Remote branches"
    } else if app.branch_tab == 2 {
        "2 Tags [ ]"
    } else {
        "2 Local branches [ ]"
    };
    render_list(
        frame,
        area,
        title.into(),
        app.focus == panel,
        items,
        (!range.is_empty()).then_some(selected.saturating_sub(range.start)),
    );
}

fn draw_stashes(frame: &mut Frame, area: Rect, app: &mut App) {
    if area.height < 2 {
        return;
    }
    let indices = app.filtered_stashes();
    let selected = app.selected(Panel::Stash);
    let range = viewport(
        app,
        Panel::Stash,
        indices.len(),
        selected,
        area.height.saturating_sub(2) as usize,
    );
    let items = indices[range.clone()]
        .iter()
        .map(|index| {
            let stash = &app.snapshot.stashes[*index];
            Line::from(vec![
                Span::styled(
                    format!("{} ", stash.reference),
                    Style::default().fg(Color::Magenta),
                ),
                Span::raw(clean(&stash.subject)),
            ])
        })
        .collect();
    render_list(
        frame,
        area,
        "4 Stashes".into(),
        app.focus == Panel::Stash,
        items,
        (!range.is_empty()).then_some(selected.saturating_sub(range.start)),
    );
}

struct GraphLine<'a> {
    prefix: &'a str,
    commit_index: Option<usize>,
    logical: Option<usize>,
}

fn graph_lines(app: &App) -> Vec<GraphLine<'_>> {
    let mut rows = vec![GraphLine {
        prefix: "◎ ",
        commit_index: None,
        logical: Some(0),
    }];
    let searching = !app.search(Panel::Commits).is_empty();
    for (logical, index) in app.filtered_commits().into_iter().enumerate() {
        let commit = &app.snapshot.commits[index];
        if !searching {
            rows.extend(commit.graph_before.iter().map(|prefix| GraphLine {
                prefix,
                commit_index: None,
                logical: None,
            }));
        }
        rows.push(GraphLine {
            prefix: if searching || commit.graph_prefix.is_empty() {
                "* "
            } else {
                &commit.graph_prefix
            },
            commit_index: Some(index),
            logical: Some(logical + 1),
        });
    }
    rows
}

fn graph_spans(text: &str) -> Vec<Span<'static>> {
    let mut result = Vec::new();
    let mut rest = text;
    let mut color = Color::Gray;
    while !rest.is_empty() {
        if let Some(tail) = rest.strip_prefix("\x1b[") {
            if let Some((codes, after)) = tail.split_once('m') {
                let codes: Vec<u16> = codes
                    .split(';')
                    .filter_map(|code| code.parse().ok())
                    .collect();
                let mut index = 0;
                while index < codes.len() {
                    color = match codes[index] {
                        0 | 39 => Color::Gray,
                        30 => Color::Black,
                        31 => Color::Red,
                        32 => Color::Green,
                        33 => Color::Yellow,
                        34 => Color::Blue,
                        35 => Color::Magenta,
                        36 => Color::Cyan,
                        37 => Color::White,
                        90 => Color::DarkGray,
                        91 => Color::LightRed,
                        92 => Color::LightGreen,
                        93 => Color::LightYellow,
                        94 => Color::LightBlue,
                        95 => Color::LightMagenta,
                        96 => Color::LightCyan,
                        97 => Color::White,
                        38 if codes.get(index + 1) == Some(&5) && codes.len() > index + 2 => {
                            index += 2;
                            Color::Indexed(codes[index].min(255) as u8)
                        }
                        38 if codes.get(index + 1) == Some(&2) && codes.len() > index + 4 => {
                            let color = Color::Rgb(
                                codes[index + 2].min(255) as u8,
                                codes[index + 3].min(255) as u8,
                                codes[index + 4].min(255) as u8,
                            );
                            index += 4;
                            color
                        }
                        _ => color,
                    };
                    index += 1;
                }
                rest = after;
                continue;
            }
            break;
        }
        let next = rest.find('\x1b').unwrap_or(rest.len());
        if next == 0 {
            rest = &rest[1..];
            continue;
        }
        result.push(Span::styled(
            rest[..next].to_owned(),
            Style::default().fg(color),
        ));
        rest = &rest[next..];
    }
    result
}

fn draw_graph(frame: &mut Frame, area: Rect, app: &mut App) {
    if area.height < 2 || area.width < 2 {
        return;
    }
    let selected_logical = if app.graph_working_tree {
        0
    } else {
        app.selected(Panel::Commits) + 1
    };
    let rows = graph_lines(app);
    let selected_visual = rows
        .iter()
        .position(|row| row.logical == Some(selected_logical))
        .unwrap_or(0);
    let total = rows.len();
    // Store positions independently of borrowed graph text before mutating viewport state.
    let height = area.height.saturating_sub(2).max(1) as usize;
    let mut offset = app.scrolls[3].min(total.saturating_sub(height));
    if selected_visual < offset {
        offset = selected_visual;
    }
    if selected_visual >= offset + height {
        offset = selected_visual + 1 - height;
    }
    let mut items = Vec::with_capacity(height);
    for row in rows.iter().skip(offset).take(height) {
        let mut spans = graph_spans(row.prefix);
        if row.logical == Some(0) {
            let changed = app
                .snapshot
                .files
                .iter()
                .filter(|file| file.changed())
                .count();
            spans.push(Span::styled(
                format!("Working tree / Index ({changed})"),
                Style::default().fg(if changed > 0 { Color::Yellow } else { MUTED }),
            ));
        } else if let Some(index) = row.commit_index {
            let commit = &app.snapshot.commits[index];
            if app.copied_commits.contains(&commit.oid) {
                spans.push(Span::styled("C ", Style::default().fg(Color::Magenta)));
            }
            spans.push(Span::styled(
                format!("{} ", commit.short),
                Style::default().fg(Color::Yellow),
            ));
            if !commit.decorations.is_empty() {
                let used: usize = spans.iter().map(Span::width).sum();
                let remaining = (area.width as usize).saturating_sub(used + 4);
                let budget = (remaining / 3).min(26);
                if budget >= 5 {
                    let labels = clean(&commit.decorations).replace("HEAD -> ", "");
                    spans.push(Span::styled(
                        format!("[{}] ", elide(&labels, budget - 3)),
                        Style::default().fg(ACCENT),
                    ));
                }
            }
            spans.push(Span::styled(
                clean(&commit.subject),
                Style::default().fg(Color::White),
            ));
        }
        items.push(Line::from(spans));
    }
    app.scrolls[3] = offset;
    let title = if app.commit_tab == 1 {
        "5 Reflog".to_owned()
    } else if let Some(path) = &app.options.history_path {
        format!("5 Graph · {}", clean(&path.to_string_lossy()))
    } else {
        format!(
            "5 Commit graph{} · {}",
            if app.loading { " …" } else { "" },
            app.snapshot.commits.len()
        )
    };
    render_list(
        frame,
        area,
        title,
        app.focus == Panel::Commits,
        items,
        Some(selected_visual.saturating_sub(offset)),
    );
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

fn draw_files(frame: &mut Frame, area: Rect, app: &mut App) {
    if area.height < 2 || area.width < 2 {
        return;
    }
    let panel = app.file_panel();
    let selected = app.selected(panel);
    let total = if panel == Panel::Files {
        app.rows.len()
    } else {
        app.commit_rows.len()
    };
    let range = viewport(
        app,
        panel,
        total,
        selected,
        area.height.saturating_sub(2) as usize,
    );
    let rows = if panel == Panel::Files {
        &app.rows
    } else {
        &app.commit_rows
    };
    let items = rows[range.clone()]
        .iter()
        .map(|row| file_line(row, app))
        .collect();
    let title = if app.graph_working_tree {
        format!(
            "7 {}{}",
            if app.changed_only {
                "Changed files"
            } else {
                "Repository files"
            },
            if app.state.show_ignored {
                " +ignored"
            } else {
                ""
            }
        )
    } else {
        format!(
            "7 Changed files · {}{}",
            &app.commit_oid[..app.commit_oid.len().min(8)],
            if app.inspection_loading { " …" } else { "" }
        )
    };
    render_list(
        frame,
        area,
        title,
        app.focus == panel,
        items,
        (!range.is_empty()).then_some(selected.saturating_sub(range.start)),
    );
    if total == 0 && area.height > 3 {
        let text = if app.inspection_loading {
            "Loading changed files…"
        } else if app.graph_working_tree {
            "Working tree is clean.\nCtrl+s changes the file filter."
        } else {
            "No changed files."
        };
        frame.render_widget(
            Paragraph::new(text)
                .style(Style::default().fg(MUTED))
                .wrap(Wrap { trim: false }),
            area.inner(ratatui::layout::Margin::new(2, 1)),
        );
    }
}

fn draw_details(frame: &mut Frame, area: Rect, app: &mut App) {
    if area.height < 2 || area.width < 2 {
        return;
    }
    let lines: Vec<_> = app
        .details_lines
        .iter()
        .map(|line| Line::raw(clean(line)))
        .collect();
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    app.details_max_scroll = paragraph
        .line_count(area.width.saturating_sub(2))
        .saturating_sub(area.height.saturating_sub(2) as usize);
    app.scrolls[9] = app.scrolls[9].min(app.details_max_scroll);
    frame.render_widget(
        paragraph
            .scroll((app.scrolls[9].min(u16::MAX as usize) as u16, 0))
            .block(block("6 Commit details", app.focus == Panel::Details)),
        area,
    );
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
        "0 Diff · {}{}{}",
        if app.preview.title.is_empty() {
            "select a file"
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
        Paragraph::new(lines).block(block("8 Command output", app.focus == Panel::Output)),
        area,
    );
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    if area.width < 80 || area.height < 22 {
        frame.render_widget(
            Paragraph::new("Enlarge the terminal to at least 80 × 22.\nPress q to quit."),
            area,
        );
        return;
    }
    let areas = areas(area, app);
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
        format!(" {}ms refresh", app.snapshot.elapsed_ms)
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
                    format!("  {} · {}", clean(&name), clean(&app.snapshot.branch)),
                    Style::default().fg(Color::White),
                ),
                Span::styled(
                    if app.snapshot.operation.is_empty() {
                        String::new()
                    } else {
                        format!(" · {}", clean(&app.snapshot.operation))
                    },
                    Style::default().fg(Color::Yellow),
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
    draw_repositories(frame, areas.repositories, app);
    draw_refs(frame, areas.local, app, false);
    draw_refs(frame, areas.remote, app, true);
    draw_stashes(frame, areas.stash, app);
    draw_graph(frame, areas.graph, app);
    draw_details(frame, areas.details, app);
    draw_files(frame, areas.files, app);
    draw_diff(frame, areas.diff, app);
    draw_output(frame, areas.output, app);
    let context_help = match app.focus {
        Panel::Files => "space stage · c commit · a stage all · F history · Ctrl+s filters",
        Panel::CommitFiles => "Enter full screen · F history · Ctrl+t difftool · c restore",
        Panel::Branches | Panel::RemoteBranches => {
            "space graph on/off · Enter checkout · n new · M merge · r rebase"
        }
        Panel::Commits if app.graph_working_tree => {
            "j/k history · Enter files · c commit · a stage all"
        }
        Panel::Commits => "j/k inspect commit · Enter files · C copy · V cherry-pick · i rebase",
        Panel::Details => "j/k scroll message · Enter files · + zoom",
        Panel::Stash => "space apply · g pop · d drop · Enter diff",
        Panel::Diff => "space stage/unstage · ←/→ hunks · a hunk/line · Esc return",
        Panel::Output => "j/k scroll · End follow output · Ctrl+c cancel",
        Panel::Status => "Enter open repository · r repository actions · Ctrl+r recent",
    };
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
                format!(" {context_help} · Tab panes · ? help"),
                Style::default().fg(ACCENT),
            ),
        ]),
        areas.footer,
    );
    draw_modal(frame, app);
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
            frame.render_widget(Paragraph::new(format!("{title}\n\nThis operation changes the repository.\n\ny / Enter confirm · n / Esc cancel")).wrap(Wrap { trim: false }).block(block("Confirm", true)), rect);
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
                    .block(block("Keys · j/k scroll · Esc close", true)),
                rect,
            );
        }
        Modal::Message(message) => {
            let rect = centered(area, 96, 12);
            frame.render_widget(Clear, rect);
            frame.render_widget(
                Paragraph::new(format!("{message}\n\nEnter / Esc close"))
                    .wrap(Wrap { trim: false })
                    .block(block("Message", true)),
                rect,
            );
        }
    }
}

pub fn hit_test(area: Rect, app: &App, x: u16, y: u16) -> Option<(Panel, usize)> {
    let areas = areas(area, app);
    for panel in [
        Panel::Status,
        Panel::Branches,
        Panel::RemoteBranches,
        Panel::Stash,
        Panel::Commits,
        Panel::Details,
        app.file_panel(),
        Panel::Diff,
        Panel::Output,
    ] {
        let rect = areas.pane(panel);
        if rect.width < 3
            || rect.height < 3
            || !rect
                .inner(ratatui::layout::Margin::new(1, 1))
                .contains((x, y).into())
        {
            continue;
        }
        let index = y.saturating_sub(rect.y + 1) as usize;
        if panel == Panel::Commits {
            let rows = graph_lines(app);
            return rows
                .get(app.scrolls[3] + index)
                .and_then(|row| row.logical)
                .map(|logical| (panel, logical));
        }
        return Some((
            panel,
            match panel {
                Panel::Diff => app.diff_scroll + index,
                Panel::Output => app.output_scroll + index,
                Panel::Details => app.scrolls[9] + index,
                _ => app.scrolls[panel_index(panel)] + index,
            },
        ));
    }
    None
}

pub const HELP: &str = "Numbered panes\n  1 Repositories · 2 Local branches · 3 Remote branches · 4 Stashes\n  5 Commit graph · 6 Commit details · 7 Changed files · 0 Diff · 8 Output\n  Number keys focus panes · Tab/Shift+Tab cycle · + zoom focused pane\n  j/k or arrows move · Home/End/PageUp/PageDown scroll · / search\n  Esc returns · q quits · Ctrl+r recent repos · @ show/hide output\n\nBranches and graph\n  Space toggles a branch in the graph; this does not check it out.\n  Enabled local and remote branches are combined with shared ancestors.\n  Enter checks out the selected branch · * marks the current branch.\n  Branch visibility is saved separately for each repository.\n  j/k in the graph updates details and files without moving focus.\n  Working tree / Index is the first graph row for uncommitted changes.\n  Enter in graph focuses files · Enter on a file opens its diff full screen; Esc returns to files.\n  [ / ] in local branches switches branches/tags/worktrees.\n  [ / ] in graph switches commits/reflog · . loads 300 more commits.\n\nWorking files\n  Space stage/unstage · a stage all · c commit · C editor · A amend\n  D reset menu (including submodule stashing/reset) · d discard\n  s stash with untracked · S stash options · i ignore · I show ignored\n  Grave key tree/flat · - collapse · = expand · v mark · Shift+arrows range\n  Ctrl+s file/history filters; turn off changed-only to browse clean files\n  F file history follows renames · e editor · o opener · y copy path\n  Ctrl+t configured difftool · Alt+m mergetool · M conflict menu\n\nDiff\n  ~ saves automatic previews on/off · 0 explicitly opens the full diff\n  Automatic file limit 2 MiB and output limit 512 KiB (configurable)\n  Left/right hunks · j/k lines · a hunk/line mode · Space stage/unstage\n  d discard selected change · [ / ] staged/unstaged · { / } context\n  H/L horizontal scroll · Ctrl+w whitespace · n/N next file or search match\n\nHistory and operations\n  Graph: g reset · t revert · C copy · V cherry-pick · F fixup · T tag\n  i interactive rebase · r/R reword/rebase editor · b bisect · y copy hash\n  Branches: n new · c checkout by name · R rename · M merge · r rebase\n  F force checkout · T tag · u upstream · w worktree · d delete\n  Stashes: Space apply · g pop · d drop · Enter diff · s save\n  p pull · P push · R refresh · m continue/abort/skip operation\n  : captured command with live output · ! interactive terminal command\n  Ctrl+c cancels running command · End follows output\n";
