use crate::{
    actions,
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
use std::{borrow::Cow, time::Duration};

const ACCENT: Color = Color::Cyan;
const MUTED: Color = Color::DarkGray;
const SELECTED: Color = Color::Rgb(35, 55, 65);

fn activity_spinner(elapsed: Duration) -> char {
    ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧'][elapsed.as_millis() as usize / 100 % 8]
}

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
            Panel::Files | Panel::CommitFiles | Panel::HeadFiles => self.files,
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
            Panel::Files | Panel::CommitFiles | Panel::HeadFiles => result.files = vertical[1],
            Panel::Diff => result.diff = vertical[1],
            Panel::Output => result.output = vertical[1],
        }
        return result;
    }
    let sidebar_width = (area.width as f64 * app.config.gui.side_panel_width)
        .round()
        .clamp(19.0, 32.0) as u16;
    let columns = Layout::horizontal([Constraint::Length(sidebar_width), Constraint::Min(1)])
        .split(vertical[1]);
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
    if app.details_panel() == Panel::Output {
        result.output = inspector[0];
    } else {
        result.details = inspector[0];
    }
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

fn clean_viewport(text: &str, offset: usize, width: usize) -> Cow<'_, str> {
    let end = offset.saturating_add(width).min(text.len());
    let printable = |byte: &u8| byte.is_ascii_graphic() || *byte == b' ';
    // A printable ASCII boundary also closes the final visible grapheme. Keep
    // the existing Unicode/escape handling whenever that cannot be established.
    if text.as_bytes()[..end].iter().all(printable)
        && text.as_bytes().get(end).is_none_or(printable)
    {
        return Cow::Borrowed(&text[offset.min(end)..end]);
    }
    Cow::Owned(clean(text).chars().skip(offset).collect())
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
        app.repository_count(),
        selected,
        area.height.saturating_sub(2) as usize,
    );
    let items = if app.repo_tab == 0 {
        app.state.recent_repos[range.clone()]
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
            .collect()
    } else {
        let parent = usize::from(app.repo.superproject.is_some());
        range
            .clone()
            .map(|index| {
                if index < parent {
                    return Line::from(vec![
                        Span::styled("↑ ", Style::default().fg(ACCENT)),
                        Span::raw("Parent repository"),
                    ]);
                }
                let module = &app.snapshot.submodules[index - parent];
                Line::from(vec![
                    Span::styled(
                        if module.initialized { "● " } else { "○ " },
                        Style::default().fg(if !module.initialized {
                            MUTED
                        } else if module.changed {
                            Color::Yellow
                        } else {
                            ACCENT
                        }),
                    ),
                    Span::raw(clean(&module.path.to_string_lossy())),
                    Span::styled(
                        if !module.initialized {
                            " · initialize"
                        } else if module.changed {
                            " · changed"
                        } else {
                            ""
                        },
                        Style::default().fg(MUTED),
                    ),
                ])
            })
            .collect()
    };
    let title = if app.repo_tab == 0 {
        format!(
            "1 [Repositories] | Submodules ({})",
            app.snapshot.submodules.len()
        )
    } else {
        format!(
            "1 Repositories | [Submodules] ({})",
            app.snapshot.submodules.len()
        )
    };
    render_list(
        frame,
        area,
        title,
        app.focus == Panel::Status,
        items,
        (!range.is_empty()).then_some(selected.saturating_sub(range.start)),
    );
    if app.repo_tab == 1 && app.repository_count() == 0 && area.height > 3 {
        frame.render_widget(
            Paragraph::new("No submodules in this repository.")
                .style(Style::default().fg(MUTED))
                .wrap(Wrap { trim: false }),
            area.inner(ratatui::layout::Margin::new(1, 1)),
        );
    }
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
    let query = crate::fuzzy::Query::new(app.search(panel));
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
    let total = app.branch_rows(remote).len();
    let range = viewport(
        app,
        panel,
        total,
        selected,
        area.height.saturating_sub(2) as usize,
    );
    let items = app.branch_rows(remote)[range.clone()]
        .iter()
        .map(|row| {
            let enabled = row
                .descendants
                .iter()
                .filter(|index| app.branch_visible(&app.snapshot.references[**index]))
                .count();
            let all = enabled == row.descendants.len();
            let expanded = !app.collapsed_branches[usize::from(remote)].contains(&row.path)
                || !app.search(panel).is_empty();
            let reference = row
                .reference
                .and_then(|index| app.snapshot.references.get(index));
            let tracking = reference
                .filter(|reference| !remote && reference.kind == crate::model::RefKind::Branch)
                .and_then(|reference| reference.ahead.zip(reference.behind))
                .map(|(ahead, behind)| format!(" ↑{ahead} ↓{behind}"))
                .unwrap_or_default();
            let name = format!(
                "{}{}",
                clean(&row.label),
                if row.is_group() {
                    app.config.gui.branch_group_delimiter.as_str()
                } else {
                    ""
                }
            );
            let name = if tracking.is_empty() {
                name
            } else {
                let reserved = row.depth * 2
                    + 6
                    + usize::from(reference.is_some_and(|reference| reference.current)) * 2
                    + Line::raw(&tracking).width();
                elide(
                    &name,
                    (area.width.saturating_sub(4) as usize).saturating_sub(reserved),
                )
            };
            let name_style = Style::default()
                .fg(if remote { Color::Yellow } else { Color::Green })
                .add_modifier(
                    if row
                        .descendants
                        .iter()
                        .any(|index| app.snapshot.references[*index].current)
                    {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    },
                );
            let label_start = row
                .path
                .rfind(&row.label)
                .map_or(0, |byte_index| row.path[..byte_index].chars().count());
            let name_matches = query
                .positions(&row.path)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|position| {
                    position
                        .checked_sub(label_start)
                        .filter(|position| *position < name.chars().count())
                })
                .collect::<Vec<_>>();
            let mut spans = vec![
                Span::raw("  ".repeat(row.depth)),
                Span::styled(
                    if all {
                        "[x] "
                    } else if enabled > 0 {
                        "[-] "
                    } else {
                        "[ ] "
                    },
                    Style::default().fg(if enabled > 0 { ACCENT } else { MUTED }),
                ),
                Span::styled(
                    if row.is_group() {
                        if expanded { "▾ " } else { "▸ " }
                    } else {
                        "  "
                    },
                    Style::default().fg(MUTED),
                ),
            ];
            spans.extend(highlighted_spans(
                vec![Span::styled(name, name_style)],
                &name_matches,
            ));
            spans.extend([
                Span::styled(
                    if reference.is_some_and(|reference| reference.current) {
                        " *"
                    } else {
                        ""
                    },
                    Style::default().fg(ACCENT),
                ),
                Span::styled(
                    if row.is_group() {
                        format!(" ({})", row.descendants.len())
                    } else {
                        String::new()
                    },
                    Style::default().fg(MUTED),
                ),
                Span::styled(tracking, Style::default().fg(MUTED)),
            ]);
            Line::from(spans)
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
    prefix: Cow<'a, str>,
    commit_index: Option<usize>,
    logical: Option<usize>,
}

fn graph_line(app: &App, row: crate::history::GraphRow) -> GraphLine<'_> {
    use crate::history::GraphRow;
    let searching = !app.search(Panel::Commits).is_empty();
    match row {
        GraphRow::WorkingTree {
            commit_index,
            logical,
        } => GraphLine {
            prefix: commit_index
                .and_then(|index| app.snapshot.commits.get(index))
                .filter(|commit| !searching && !commit.graph_prefix.is_empty())
                .map(|commit| Cow::Owned(commit.graph_prefix.replace('*', "◎")))
                .unwrap_or(Cow::Borrowed("◎ ")),
            commit_index: None,
            logical: Some(logical),
        },
        GraphRow::Commit {
            commit_index,
            logical,
        } => {
            let commit = &app.snapshot.commits[commit_index];
            GraphLine {
                prefix: if searching || commit.graph_prefix.is_empty() {
                    Cow::Borrowed("* ")
                } else {
                    Cow::Borrowed(&commit.graph_prefix)
                },
                commit_index: Some(commit_index),
                logical: Some(logical),
            }
        }
        GraphRow::Connector { commit_index, line } => GraphLine {
            prefix: Cow::Borrowed(&app.snapshot.commits[commit_index].graph_before[line]),
            commit_index: None,
            logical: None,
        },
    }
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

fn initials(name: &str) -> String {
    let mut words = name
        .split_whitespace()
        .filter_map(|word| word.chars().find(|character| character.is_alphanumeric()));
    let Some(first) = words.next() else {
        return "??".into();
    };
    let last = words.next_back();
    std::iter::once(first)
        .chain(last)
        .flat_map(char::to_uppercase)
        .collect()
}

fn short_datetime(timestamp: &str) -> String {
    let Some(date) = timestamp.get(..10) else {
        return "—".into();
    };
    let mut fields = date.split('-');
    let (Some(year), Some(month), Some(day)) = (
        fields.next(),
        fields.next().and_then(|value| value.parse::<usize>().ok()),
        fields.next().and_then(|value| value.parse::<u8>().ok()),
    ) else {
        return "—".into();
    };
    let months = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    if !(1..=31).contains(&day) {
        return "—".into();
    }
    let Some(month) = months.get(month.wrapping_sub(1)) else {
        return "—".into();
    };
    let mut result = format!("{day:02} {month} {year}");
    if let Some(time) = timestamp.get(11..16) {
        result.push(' ');
        result.push_str(time);
    }
    result
}

fn highlighted_spans(spans: Vec<Span<'static>>, positions: &[usize]) -> Vec<Span<'static>> {
    if positions.is_empty() {
        return spans;
    }
    let mut result = Vec::new();
    let mut offset = 0;
    for span in spans {
        let mut text = String::new();
        let mut highlighted = false;
        for (index, character) in span.content.chars().enumerate() {
            let matched = positions.binary_search(&(offset + index)).is_ok();
            if !text.is_empty() && matched != highlighted {
                result.push(Span::styled(
                    std::mem::take(&mut text),
                    if highlighted {
                        span.style
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
                    } else {
                        span.style
                    },
                ));
            }
            highlighted = matched;
            text.push(character);
        }
        offset += span.content.chars().count();
        if !text.is_empty() {
            result.push(Span::styled(
                text,
                if highlighted {
                    span.style
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
                } else {
                    span.style
                },
            ));
        }
    }
    result
}

fn highlighted_elide(
    text: &str,
    width: usize,
    positions: &[usize],
    style: Style,
) -> Vec<Span<'static>> {
    let displayed = elide(text, width);
    let retained = displayed
        .chars()
        .count()
        .saturating_sub(usize::from(displayed != text));
    let positions: Vec<_> = positions
        .iter()
        .copied()
        .filter(|index| *index < retained)
        .collect();
    highlighted_spans(vec![Span::styled(displayed, style)], &positions)
}

fn fit_spans(spans: Vec<Span<'static>>, width: usize) -> Vec<Span<'static>> {
    let mut result = Vec::new();
    let mut remaining = width;
    for span in spans {
        let used = span.width();
        if used > remaining {
            if remaining > 0 {
                result.push(Span::styled(elide(&span.content, remaining), span.style));
            }
            break;
        }
        remaining -= used;
        result.push(span);
    }
    result
}
fn draw_graph(frame: &mut Frame, area: Rect, app: &mut App) {
    if area.height < 2 || area.width < 2 {
        return;
    }
    let selected_logical = app.selected_graph_row();
    let index = app.graph_index();
    let working_tree_index = app.working_tree_graph_index();
    let selected_visual = index
        .visual_for_logical(selected_logical, working_tree_index)
        .unwrap_or(0);
    let total = index.len();
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
    let content_width = area.width.saturating_sub(4) as usize;
    let query = crate::fuzzy::Query::new(app.search(Panel::Commits));
    for visual in offset..offset.saturating_add(height).min(total) {
        let row = graph_line(app, index.row(visual, working_tree_index).unwrap());
        let mut spans = graph_spans(&row.prefix);
        if row.logical.is_some() && row.commit_index.is_none() {
            let changed = app.changed_file_count();
            spans.push(Span::styled(
                elide(
                    &format!("Working tree / Index ({changed})"),
                    content_width.saturating_sub(spans.iter().map(Span::width).sum()),
                ),
                Style::default().fg(if changed > 0 { Color::Yellow } else { MUTED }),
            ));
        } else if let Some(index) = row.commit_index {
            let commit = &app.snapshot.commits[index];
            let subject = clean(&commit.subject);
            let decorations = clean(&commit.decorations);
            let subject_offset = commit.oid.chars().count() + 1;
            let (matches, decorations_offset) = if app.search(Panel::Commits).is_empty() {
                (Vec::new(), 0)
            } else {
                let author = clean(&commit.author);
                let committer = clean(&commit.committer);
                let candidate = format!(
                    "{} {subject} {author} {committer} {decorations}",
                    commit.oid
                );
                (
                    query.positions(&candidate).unwrap_or_default(),
                    subject_offset
                        + subject.chars().count()
                        + author.chars().count()
                        + committer.chars().count()
                        + 3,
                )
            };
            let subject_matches = query.positions(&subject).unwrap_or_else(|| {
                matches
                    .iter()
                    .copied()
                    .filter(|index| {
                        *index >= subject_offset
                            && *index < subject_offset + subject.chars().count()
                    })
                    .map(|index| index - subject_offset)
                    .collect()
            });
            let hash_matches: Vec<_> = matches
                .iter()
                .copied()
                .filter(|index| *index < commit.short.chars().count())
                .collect();
            let metadata = format!(
                "{} {}",
                initials(if commit.committer.is_empty() {
                    &commit.author
                } else {
                    &commit.committer
                }),
                short_datetime(if commit.committed_at.is_empty() {
                    &commit.date
                } else {
                    &commit.committed_at
                })
            );
            let metadata = elide(&metadata, content_width);
            let metadata_width = Line::raw(&metadata).width();
            let left_width = content_width.saturating_sub(metadata_width + 1);
            spans = fit_spans(spans, left_width.saturating_sub(3));
            if app.copied_commits.contains(&commit.oid) {
                let budget = left_width.saturating_sub(spans.iter().map(Span::width).sum());
                spans.push(Span::styled(
                    elide("C ", budget.min(2)),
                    Style::default().fg(Color::Magenta),
                ));
            }
            let available = left_width.saturating_sub(spans.iter().map(Span::width).sum());
            let hash_budget = available.saturating_sub(3).min(commit.short.len());
            if hash_budget > 0 {
                spans.extend(highlighted_elide(
                    &commit.short,
                    hash_budget,
                    &hash_matches,
                    Style::default().fg(Color::Yellow),
                ));
                spans.push(Span::raw(" "));
            }
            if !commit.decorations.is_empty() {
                let used: usize = spans.iter().map(Span::width).sum();
                let budget = left_width.saturating_sub(used);
                if budget >= 4 {
                    let mut source_offset = decorations_offset;
                    let mut display_offset = 0;
                    let mut label_matches = Vec::new();
                    let mut labels = decorations
                        .split(", ")
                        .enumerate()
                        .map(|(index, label)| {
                            let current = label.starts_with("HEAD -> ")
                                || (commit.oid == app.snapshot.head
                                    && label == app.snapshot.branch);
                            let prefix = if index == 0 { "" } else { ", " };
                            let visible = label.trim_start_matches("HEAD -> ");
                            let removed = label.chars().count() - visible.chars().count();
                            label_matches.extend(
                                matches
                                    .iter()
                                    .copied()
                                    .filter(|position| {
                                        *position >= source_offset + removed
                                            && *position < source_offset + label.chars().count()
                                    })
                                    .map(|position| {
                                        display_offset + prefix.chars().count() + position
                                            - source_offset
                                            - removed
                                    }),
                            );
                            source_offset += label.chars().count() + 2;
                            display_offset += prefix.chars().count() + visible.chars().count();
                            Span::styled(
                                format!("{prefix}{visible}"),
                                Style::default().fg(ACCENT).add_modifier(if current {
                                    Modifier::BOLD
                                } else {
                                    Modifier::empty()
                                }),
                            )
                        })
                        .collect::<Vec<_>>();
                    spans.push(Span::styled("[", Style::default().fg(ACCENT)));
                    let fitted = fit_spans(std::mem::take(&mut labels), budget - 3);
                    let retained = fitted
                        .iter()
                        .map(|span| span.content.chars().count())
                        .sum::<usize>()
                        .saturating_sub(usize::from(
                            fitted
                                .last()
                                .is_some_and(|span| span.content.ends_with('…')),
                        ));
                    label_matches.retain(|index| *index < retained);
                    spans.extend(highlighted_spans(fitted, &label_matches));
                    spans.push(Span::styled("] ", Style::default().fg(ACCENT)));
                }
            }
            spans.extend(highlighted_elide(
                &subject,
                left_width.saturating_sub(spans.iter().map(Span::width).sum()),
                &subject_matches,
                Style::default().fg(Color::White),
            ));
            let used: usize = spans.iter().map(Span::width).sum();
            spans.push(Span::raw(
                " ".repeat(content_width.saturating_sub(used + metadata_width)),
            ));
            spans.push(Span::styled(
                metadata,
                Style::default().fg(Color::LightBlue),
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
            "5 Commit graph{} · {}{} · {}ms load",
            if app.history_loading { " …" } else { "" },
            app.snapshot.commits.len(),
            if app.history_exhausted { "" } else { "+" },
            app.history_elapsed_ms,
        )
    };
    let title = if app.history_error.is_some() {
        format!("{title} · paused · L options")
    } else {
        title
    };
    let title = if let Some(mark) = &app.comparison_mark {
        format!("{title} · A: {}", clean(&mark.name))
    } else {
        title
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

fn file_line(
    row: &Row,
    app: &App,
    query: &crate::fuzzy::Query,
    match_path: &std::path::Path,
) -> Line<'static> {
    let color = if app.file_panel() == Panel::HeadFiles {
        Color::White
    } else if row.ignored || !row.changed {
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
    let marker = if row.directory {
        if app.collapsed.contains(&row.path) && app.search(app.file_panel()).is_empty() {
            "▸"
        } else {
            "▾"
        }
    } else {
        " "
    };
    let checkbox = app.file_checkbox(row);
    let mut spans = vec![
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
        Span::raw(" "),
        Span::styled(
            checkbox,
            Style::default().fg(if matches!(checkbox, "[x] " | "[-] ") {
                ACCENT
            } else {
                MUTED
            }),
        ),
        Span::raw(format!("{}{marker} ", "  ".repeat(row.depth))),
    ];
    let label = clean(&row.label);
    if app.search(app.file_panel()).is_empty() {
        spans.push(Span::styled(label, Style::default().fg(color)));
        return Line::from(spans);
    }
    let full_path = clean(&match_path.to_string_lossy());
    let row_path = clean(&row.path.to_string_lossy());
    let label_offset = if app.config.gui.show_file_tree {
        row_path
            .chars()
            .count()
            .saturating_sub(label.chars().count())
    } else {
        0
    };
    let positions: Vec<_> = query
        .positions(&full_path)
        .unwrap_or_default()
        .into_iter()
        .filter(|index| *index >= label_offset && *index < label_offset + label.chars().count())
        .map(|index| index - label_offset)
        .collect();
    spans.extend(highlighted_spans(
        vec![Span::styled(label, Style::default().fg(color))],
        &positions,
    ));
    Line::from(spans)
}

fn draw_files(frame: &mut Frame, area: Rect, app: &mut App) {
    if area.height < 2 || area.width < 2 {
        return;
    }
    let panel = app.file_panel();
    let selected = app.selected(panel);
    let total = app.file_rows(panel).len();
    let range = viewport(
        app,
        panel,
        total,
        selected,
        area.height.saturating_sub(2) as usize,
    );
    let rows = app.file_rows(panel);
    let query = crate::fuzzy::Query::new(app.search(panel));
    let items = rows[range.clone()]
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let match_path = if row.directory && !app.search(panel).is_empty() {
                rows[range.start + index + 1..]
                    .iter()
                    .take_while(|child| child.depth > row.depth)
                    .find(|child| !child.directory)
                    .map(|child| child.path.as_path())
                    .unwrap_or(&row.path)
            } else {
                &row.path
            };
            file_line(row, app, &query, match_path)
        })
        .collect();
    let mut title = if app.stash_inspection_active() {
        format!(
            "7 [Stash] | {}{}",
            clean(&app.stash_file_title()),
            if app.stash_inspection_loading() {
                " …"
            } else {
                ""
            }
        )
    } else if panel == Panel::HeadFiles {
        format!(
            "7 Changes | [HEAD tree]{}",
            if app.head_loading { " …" } else { "" }
        )
    } else if let Some(info) = &app.comparison {
        format!(
            "7 [Compare] | HEAD tree · {}{}",
            clean(&info.title()),
            if app.comparison_loading { " …" } else { "" }
        )
    } else if app.graph_working_tree {
        format!(
            "7 [Changes] | HEAD tree{}{}",
            if app.changed_only {
                ""
            } else {
                " · all files"
            },
            if app.state.show_ignored {
                " +ignored"
            } else {
                ""
            }
        )
    } else {
        format!(
            "7 [Changes] | HEAD tree · {}{}",
            &app.commit_oid[..app.commit_oid.len().min(8)],
            if app.inspection_loading { " …" } else { "" }
        )
    };
    let selected_files = app.file_selected_count();
    if selected_files > 0 {
        title.push_str(&format!(" · {selected_files} selected · U clear"));
    }
    render_list(
        frame,
        area,
        title,
        app.focus == panel,
        items,
        (!range.is_empty()).then_some(selected.saturating_sub(range.start)),
    );
    if total == 0 && area.height > 3 {
        let text = if !app.search(panel).is_empty() {
            "No matching files."
        } else if app.stash_inspection_active() && app.stash_inspection_loading() {
            "Loading stashed files…"
        } else if app.stash_inspection_active() {
            "This stash has no changed files."
        } else if panel == Panel::HeadFiles && app.head_loading {
            "Loading HEAD tree…"
        } else if panel == Panel::HeadFiles {
            "HEAD has no files."
        } else if app.comparison_loading {
            "Loading comparison…"
        } else if app.comparison.is_some() {
            "No differences between these revisions."
        } else if app.inspection_loading {
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
            .block(block(
                if app.comparison.is_some() {
                    "6 [Comparison] | Output"
                } else {
                    "6 [Details] | Output"
                },
                app.focus == Panel::Details,
            )),
        area,
    );
}

fn draw_diff(frame: &mut Frame, area: Rect, app: &mut App) {
    if area.height < 2 || area.width < 2 {
        return;
    }
    let height = area.height.saturating_sub(2) as usize;
    app.diff_view_height = height;
    app.diff_scroll = app
        .diff_scroll
        .min(app.preview_lines.len().saturating_sub(height));
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
                clean_viewport(
                    line,
                    app.horizontal_scroll,
                    area.width.saturating_sub(2) as usize,
                ),
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
                clean_viewport(line, 0, area.width.saturating_sub(2) as usize),
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
    let title = if let Some((_, started, label)) = &app.busy {
        format!(
            "6 [Output] · {} {} · {}s · Ctrl+c cancel | Details",
            activity_spinner(started.elapsed()),
            if label.starts_with("git switch ") {
                "switching"
            } else {
                "running"
            },
            started.elapsed().as_secs(),
        )
    } else {
        "6 Details | [Output]".into()
    };
    frame.render_widget(
        Paragraph::new(lines).block(block(title, app.focus == Panel::Output)),
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
        if rect.height > 2 && rect.width > 2 {
            app.page_sizes[panel_index(panel)] = rect.height.saturating_sub(2) as usize;
        }
    }
    let name = app
        .repo
        .root
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    let activity = if let Some((_, started, label)) = &app.busy {
        format!(
            " {} {}s · {}",
            activity_spinner(started.elapsed()),
            started.elapsed().as_secs(),
            clean(label)
        )
    } else if app.loading {
        " refreshing…".into()
    } else {
        format!(" {}ms refresh", app.snapshot.elapsed_ms)
    };
    let tracking = match app.snapshot.ahead.zip(app.snapshot.behind) {
        Some((ahead, behind)) => format!(" · AHEAD {ahead} · BEHIND {behind}"),
        None if !app.snapshot.upstream.is_empty() => " · upstream unavailable".into(),
        None if !app.snapshot.branch.is_empty() => " · no upstream".into(),
        None => String::new(),
    };
    let active_branch = app
        .snapshot
        .references
        .iter()
        .find(|reference| reference.current)
        .map(|reference| reference.name.as_str())
        .unwrap_or(&app.snapshot.branch);
    let repository_label = elide(
        &format!("  {} · {}", clean(&name), clean(active_branch)),
        (area.width.saturating_sub(12) as usize).saturating_sub(Line::raw(&tracking).width()),
    );
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled(
                    " RUSTIFIEDGIT ",
                    Style::default()
                        .bg(ACCENT)
                        .fg(Color::Black)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    repository_label,
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(tracking, Style::default().fg(Color::LightBlue)),
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
    for panel in [
        Panel::Commits,
        app.file_panel(),
        Panel::Branches,
        Panel::RemoteBranches,
    ] {
        if let Some(hint) = app.search_hint(panel) {
            let rect = areas.pane(panel);
            if rect.width > 2 && rect.height > 2 {
                frame.render_widget(
                    Paragraph::new(elide(&clean(&hint), rect.width.saturating_sub(2) as usize))
                        .style(Style::default().fg(ACCENT)),
                    Rect::new(rect.x + 1, rect.bottom() - 1, rect.width - 2, 1),
                );
            }
        }
    }
    let context_actions = actions::for_footer(app);
    let context_help = context_actions
        .iter()
        .take(5)
        .map(|action| format!("{} {}", action.key, action.label))
        .collect::<Vec<_>>()
        .join(" · ");
    let menu_hint = actions::context_key(app)
        .map(|key| format!("{key} actions · "))
        .unwrap_or_default();
    let page_hint = if app.focus == Panel::Commits {
        "Shift+↑/↓ page · "
    } else if app.search_edit.is_some() {
        "Fuzzy search · ↑/↓ select · Enter keep · Esc cancel · Ctrl+u clear · space AND · 'exact · !exclude"
    } else {
        ""
    };
    let description = if let Some(Modal::Actions {
        items, selected, ..
    }) = &app.modal
    {
        items
            .get(*selected)
            .map(|action| action.description.as_str())
            .unwrap_or("")
    } else {
        &app.notice
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(
                format!(" {}", clean(description)),
                Style::default().fg(if app.busy.is_some() {
                    Color::Yellow
                } else {
                    MUTED
                }),
            ),
            Line::styled(
                format!(" {menu_hint}{page_hint}{context_help} · Tab panes · ? help"),
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

fn help_layout(area: Rect) -> (Rect, [Rect; 5]) {
    let rect = centered(area, 120, area.height.saturating_sub(2));
    let inner = rect.inner(ratatui::layout::Margin::new(1, 1));
    let parts = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(u16::from(inner.height >= 8)),
        Constraint::Min(1),
        Constraint::Length(if inner.height >= 12 {
            4
        } else if inner.height >= 8 {
            2
        } else {
            0
        }),
        Constraint::Length(u16::from(inner.height >= 4)),
    ])
    .split(inner);
    (rect, [parts[0], parts[1], parts[2], parts[3], parts[4]])
}

fn help_row(
    entry: &crate::help::Entry,
    query: &crate::fuzzy::Query,
    width: usize,
) -> (Line<'static>, Line<'static>) {
    let key_width = if width >= 80 { 22 } else { (width / 3).min(16) };
    let section_width = if width >= 70 { 18 } else { 0 };
    let label_width = width.saturating_sub(key_width + section_width + 4);
    let key = elide(&clean(&entry.keys), key_width);
    let label = elide(&clean(&entry.label), label_width);
    let mut spans = vec![
        Span::styled(
            format!("{key:>key_width$}  "),
            Style::default().fg(if entry.keys == "Disabled" {
                MUTED
            } else {
                ACCENT
            }),
        ),
        Span::raw(label.clone()),
    ];
    if section_width > 0 {
        spans.push(Span::raw(
            " ".repeat(label_width.saturating_sub(Line::raw(&label).width()) + 2),
        ));
        spans.push(Span::styled(
            elide(&clean(&entry.section), section_width),
            Style::default().fg(MUTED),
        ));
    }
    let row_text: String = spans.iter().map(|span| span.content.as_ref()).collect();
    let row_chars = row_text.chars().count();
    let description = format!("{} · {}", clean(&entry.section), clean(&entry.description));
    let positions = query
        .positions(&format!("{row_text}\n{description}"))
        .unwrap_or_default();
    let row_positions: Vec<_> = positions
        .iter()
        .copied()
        .filter(|index| *index < row_chars)
        .collect();
    let description_positions: Vec<_> = positions
        .into_iter()
        .filter_map(|index| index.checked_sub(row_chars + 1))
        .collect();
    (
        Line::from(highlighted_spans(spans, &row_positions)),
        Line::from(highlighted_spans(
            vec![Span::styled(
                description,
                Style::default().fg(Color::LightBlue),
            )],
            &description_positions,
        )),
    )
}

fn draw_help(frame: &mut Frame, area: Rect, state: &crate::help::HelpState) {
    let (rect, parts) = help_layout(area);
    let matches = state.matches();
    frame.render_widget(Clear, rect);
    frame.render_widget(
        block(
            format!(
                "Keyboard help · {} / {} commands",
                matches.len(),
                state.entries.len()
            ),
            true,
        ),
        rect,
    );
    let search_width = parts[0].width.saturating_sub(3) as usize;
    let mut visible_query = state.query.clone();
    while !visible_query.is_empty() && Line::raw(&visible_query).width() > search_width {
        visible_query.remove(0);
    }
    let search = Line::from(vec![
        Span::styled(
            "/ ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        if state.query.is_empty() {
            Span::styled(
                "Type to search commands, shortcuts or panes…",
                Style::default().fg(MUTED),
            )
        } else {
            Span::raw(visible_query.clone())
        },
    ]);
    frame.render_widget(Paragraph::new(search), parts[0]);
    if parts[0].width > 2 && parts[0].height > 0 {
        frame.set_cursor_position((
            parts[0].x + 2 + Line::raw(&visible_query).width() as u16,
            parts[0].y,
        ));
    }
    let width = parts[2].width.saturating_sub(2) as usize;
    let query = crate::fuzzy::Query::new(&state.query);
    let headings = crate::help::Entry {
        keys: "Shortcut".into(),
        label: "Command".into(),
        section: "Pane / topic".into(),
        description: String::new(),
    };
    let mut heading = help_row(&headings, &crate::fuzzy::Query::new(""), width).0;
    heading.spans.insert(0, Span::raw("  "));
    frame.render_widget(
        Paragraph::new(heading).style(Style::default().fg(MUTED).add_modifier(Modifier::BOLD)),
        parts[1],
    );
    if matches.is_empty() {
        frame.render_widget(
            Paragraph::new("No matching commands. Backspace edits · Ctrl+U clears search.")
                .style(Style::default().fg(MUTED))
                .wrap(Wrap { trim: false }),
            parts[2],
        );
    } else {
        let selected = state.selected.min(matches.len() - 1);
        let offset = selected.saturating_sub(parts[2].height.saturating_sub(1) as usize);
        let rows: Vec<_> = matches
            .iter()
            .skip(offset)
            .take(parts[2].height as usize)
            .map(|index| ListItem::new(help_row(&state.entries[*index], &query, width).0))
            .collect();
        let mut list_state = ListState::default().with_selected(Some(selected - offset));
        frame.render_stateful_widget(
            List::new(rows)
                .highlight_style(Style::default().bg(SELECTED))
                .highlight_symbol("› "),
            parts[2],
            &mut list_state,
        );
        let description = help_row(&state.entries[matches[selected]], &query, width).1;
        frame.render_widget(
            Paragraph::new(vec![Line::raw(""), description]).wrap(Wrap { trim: false }),
            parts[3],
        );
    }
    frame.render_widget(
        Paragraph::new("Type to filter · ↑↓ select · PgUp/PgDn page · Ctrl+U clear · Esc close")
            .style(Style::default().fg(Color::Yellow)),
        parts[4],
    );
}

fn draw_modal(frame: &mut Frame, app: &mut App) {
    if let Some(Modal::Help { state }) = &mut app.modal {
        state.page_size = help_layout(frame.area()).1[2].height.max(1) as usize;
    }
    let Some(modal) = &app.modal else {
        return;
    };
    let area = frame.area();
    match modal {
        Modal::Busy { running, requested } => {
            let rect = centered(area, 88, 14);
            frame.render_widget(Clear, rect);
            let inner = rect.inner(ratatui::layout::Margin::new(1, 1));
            let active = app.busy.is_some();
            frame.render_widget(
                block(
                    if active {
                        "Command already running"
                    } else {
                        "Command finished"
                    },
                    true,
                ),
                rect,
            );
            let parts = Layout::vertical([Constraint::Min(1), Constraint::Length(3)]).split(inner);
            frame.render_widget(
                Paragraph::new(format!(
                    "{}:\n{}\n\nNot started: {}",
                    if active {
                        "Another command is still running"
                    } else {
                        "The previous command has finished"
                    },
                    clean(running),
                    clean(requested)
                ))
                .wrap(Wrap { trim: false }),
                parts[0],
            );
            let guidance = if active {
                "Wait for it to finish, or Ctrl+c to cancel it.\nYour new command has not been queued.\nEnter / Esc closes this warning."
            } else {
                "Your new command has not been queued.\nEnter / Esc closes this warning; then try again."
            };
            frame.render_widget(
                Paragraph::new(guidance)
                    .style(Style::default().fg(Color::Yellow))
                    .wrap(Wrap { trim: false }),
                parts[1],
            );
        }
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
            let body = Paragraph::new(format!("{title}\n\nThis operation changes the repository."))
                .wrap(Wrap { trim: false });
            let width = area.width.saturating_sub(4).min(86);
            let height = (body.line_count(width) + 6).min(u16::MAX as usize) as u16;
            let rect = centered(area, 88, height);
            frame.render_widget(Clear, rect);
            frame.render_widget(block("Confirm", true), rect);
            let parts = Layout::vertical([Constraint::Min(1), Constraint::Length(2)])
                .split(rect.inner(ratatui::layout::Margin::new(1, 1)));
            frame.render_widget(body, parts[0]);
            frame.render_widget(
                Paragraph::new("y / Enter confirm · n / Esc cancel")
                    .style(Style::default().fg(Color::Yellow))
                    .wrap(Wrap { trim: false }),
                parts[1],
            );
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
        Modal::Help { state } => draw_help(frame, area, state),
        Modal::Actions {
            title,
            items,
            selected,
        } => {
            let rect = centered(
                area,
                104,
                (items.len() + 7).min(area.height.saturating_sub(4) as usize) as u16,
            );
            frame.render_widget(Clear, rect);
            let border = block(title.clone(), true);
            let inner = border.inner(rect);
            frame.render_widget(border, rect);
            let regions = Layout::vertical([
                Constraint::Min(1),
                Constraint::Length(3),
                Constraint::Length(1),
            ])
            .split(inner);
            let lines: Vec<_> = items
                .iter()
                .map(|item| {
                    ListItem::new(Line::from(vec![
                        Span::styled(format!("{:>12}  ", item.key), Style::default().fg(ACCENT)),
                        Span::raw(clean(&item.label)),
                    ]))
                })
                .collect();
            let mut state = ListState::default().with_selected(Some(*selected));
            frame.render_stateful_widget(
                List::new(lines)
                    .highlight_style(Style::default().bg(SELECTED))
                    .highlight_symbol("› "),
                regions[0],
                &mut state,
            );
            let description = items
                .get(*selected)
                .map(|item| clean(&item.description))
                .unwrap_or_default();
            frame.render_widget(
                Paragraph::new(description)
                    .wrap(Wrap { trim: false })
                    .style(Style::default().fg(Color::LightBlue)),
                regions[1],
            );
            frame.render_widget(
                Paragraph::new("↑/↓ choose · Enter or shortcut runs · Esc closes")
                    .style(Style::default().fg(MUTED)),
                regions[2],
            );
        }
        Modal::Message(message) => {
            let paragraph = Paragraph::new(message.as_str()).wrap(Wrap { trim: false });
            let width = area.width.saturating_sub(2).min(96);
            let lines = paragraph.line_count(width.saturating_sub(2));
            let rect = centered(area, 96, lines.saturating_add(4).clamp(7, 24) as u16);
            frame.render_widget(Clear, rect);
            let regions = Layout::vertical([Constraint::Min(1), Constraint::Length(2)])
                .split(rect.inner(ratatui::layout::Margin::new(1, 1)));
            app.message_page_size = regions[0].height as usize;
            app.message_max_scroll = lines
                .saturating_sub(app.message_page_size)
                .min(u16::MAX as usize);
            app.message_scroll = app.message_scroll.min(app.message_max_scroll);
            let title = if app.message_max_scroll > 0 {
                format!(
                    "Message · lines {}–{} of {lines}",
                    app.message_scroll + 1,
                    (app.message_scroll + app.message_page_size).min(lines)
                )
            } else {
                "Message".into()
            };
            frame.render_widget(block(title, true), rect);
            frame.render_widget(paragraph.scroll((app.message_scroll as u16, 0)), regions[0]);
            frame.render_widget(
                Paragraph::new(if app.message_max_scroll > 0 {
                    "↑/↓ or PgUp/PgDn scroll · Enter / Esc close"
                } else {
                    "Enter / Esc close"
                })
                .style(Style::default().fg(Color::Yellow))
                .wrap(Wrap { trim: false }),
                regions[1],
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
            let rows = app.graph_index();
            return rows
                .row(app.scrolls[3] + index, app.working_tree_graph_index())
                .and_then(crate::history::GraphRow::logical)
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

#[cfg(test)]
mod viewport_tests {
    use super::*;
    use ratatui::{buffer::Buffer, widgets::Widget};

    #[test]
    fn clipped_text_preserves_sanitizing_scrolling_and_grapheme_boundaries() {
        let long_ascii = "abc xyz ".repeat(100_000);
        let texts = [
            long_ascii.as_str(),
            "plain text",
            "e\u{301}x",
            "abcde\u{301}f",
            "abc👩‍💻xyz",
            "界界abc",
            "a\tbb\r\nend",
            "a\x1b[31mbb\x1b[0m界end",
            "a\x1b]0;title\x07\u{301}bc",
            "x\x1b[0m\u{301}abc",
            "",
        ];
        for text in texts {
            for offset in [0, 1, 4, 20, text.len() + 1] {
                for width in [1, 2, 5, 12, 80] {
                    let area = Rect::new(0, 0, width, 1);
                    let mut expected = Buffer::empty(area);
                    let mut actual = Buffer::empty(area);
                    Paragraph::new(clean(text).chars().skip(offset).collect::<String>())
                        .render(area, &mut expected);
                    Paragraph::new(clean_viewport(text, offset, width as usize))
                        .render(area, &mut actual);
                    assert_eq!(actual, expected, "offset {offset}, width {width}");
                }
            }
        }
        assert!(matches!(
            clean_viewport(&long_ascii, 10, 80),
            Cow::Borrowed(_)
        ));
    }
}
