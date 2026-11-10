use crate::{
    actions::{self, Action},
    branches::{self, BranchRow},
    config::{Config, CustomCommand, MAX_HISTORY_PAGE_SIZE, State, key_matches},
    git::{Operation, ReadOptions, Repository, Step, discard_operation, stage_operation},
    history::{CommitOrder, GraphIndex},
    model::{
        CommitDetails, ComparisonInfo, ComparisonRequest, FileEntry, Panel, Preview, PreviewTarget,
        RefKind, Revision, Snapshot,
    },
    patch::Patch,
    process::{Cancellation, CommandSpec},
    tree::{self, Row, fuzzy_match},
    worker::{CommandRequest, PreviewRequest, WorkerEvent, Workers},
};
use anyhow::{Context, Result, bail};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::{
    cell::RefCell,
    collections::{HashSet, VecDeque},
    env,
    ffi::OsString,
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

#[derive(Clone)]
pub enum Prompt {
    Search,
    Commit(bool),
    Branch(String),
    RenameBranch(String),
    CheckoutBranch,
    Tag(String),
    Stash,
    Shell(bool),
    HistoryPath,
    HistoryLimit,
    CompareFrom,
    CompareTo(Revision),
    CompareBase(Revision),
    Worktree(String),
    Custom {
        command: CustomCommand,
        values: Vec<String>,
        index: usize,
    },
}

#[derive(Clone)]
pub enum MenuAction {
    Run(Operation, bool),
    PullMenu,
    PushMenu(Option<crate::model::Reference>),
    External(CommandSpec),
    Input(Prompt),
    SwitchRepo(PathBuf),
    HistoryClear,
    HistoryLimit(usize),
    HistoryMore,
    HistoryRetry,
    ToggleLog,
    ToggleUnchanged,
    ToggleFilter,
    Copy(String),
    CompareMark(Revision),
    Compare(ComparisonRequest),
    CompareClose,
    CompareUnmark,
}

#[derive(Clone)]
pub struct MenuItem {
    pub label: String,
    pub action: MenuAction,
}

pub enum Modal {
    Busy {
        running: String,
        requested: String,
    },
    Input {
        title: String,
        text: String,
        purpose: Prompt,
    },
    Confirm {
        title: String,
        operation: Operation,
    },
    Menu {
        title: String,
        items: Vec<MenuItem>,
        selected: usize,
    },
    Actions {
        title: String,
        items: Vec<Action>,
        selected: usize,
    },
    Help {
        scroll: usize,
    },
    Message(String),
}

pub struct SearchEdit {
    pub panel: Panel,
    previous: String,
    selection: usize,
    scroll: usize,
    working_tree: bool,
    branch_row: Option<(String, bool)>,
}

#[derive(Default)]
struct CommitSearchCache {
    commits: Arc<Vec<crate::model::Commit>>,
    query: String,
    matches: CommitOrder,
    head: String,
    working_tree: usize,
}

pub struct App {
    pub repo: Repository,
    pub config: Config,
    pub state: State,
    pub snapshot: Arc<Snapshot>,
    pub workers: Workers,
    pub focus: Panel,
    pub side: Panel,
    pub rows: Vec<Row>,
    pub commit_rows: Vec<Row>,
    pub commit_files: Vec<FileEntry>,
    pub commit_oid: String,
    stash_rows: Vec<Row>,
    stash_files: Vec<FileEntry>,
    stash_inspection_oid: Option<String>,
    stash_inspection_loading: bool,
    stash_context: bool,
    pub comparison: Option<ComparisonInfo>,
    pub comparison_loading: bool,
    pub comparison_mark: Option<Revision>,
    pub head_tree_tab: bool,
    pub head_files: Vec<FileEntry>,
    pub head_rows: Vec<Row>,
    pub head_oid: String,
    pub head_loading: bool,
    head_requested: Option<String>,
    pub selections: [usize; 11],
    pub scrolls: [usize; 11],
    pub searches: [String; 11],
    pub page_sizes: [usize; 11],
    pub search_edit: Option<SearchEdit>,
    commit_search_cache: RefCell<CommitSearchCache>,
    graph_index_cache: RefCell<Option<Arc<GraphIndex>>>,
    pub branch_rows_cache: [Vec<BranchRow>; 2],
    pub collapsed_branches: [HashSet<String>; 2],
    pub graph_working_tree: bool,
    pub commit_details: Option<CommitDetails>,
    pub details_lines: Vec<String>,
    pub details_max_scroll: usize,
    pub inspection_loading: bool,
    inspection_oid: Option<String>,
    pub collapsed: HashSet<PathBuf>,
    pub marked: HashSet<PathBuf>,
    pub marked_commit_files: HashSet<PathBuf>,
    pub copied_commits: Vec<String>,
    pub branch_tab: usize,
    pub commit_tab: usize,
    pub preview: Preview,
    pub preview_lines: Vec<String>,
    pub patch: Patch,
    pub diff_line: usize,
    pub diff_scroll: usize,
    pub diff_view_height: usize,
    pub horizontal_scroll: usize,
    pub hunk_mode: bool,
    pub diff_context: usize,
    pub ignore_whitespace: bool,
    pub show_staged: bool,
    pub output: VecDeque<String>,
    output_pending: String,
    output_overwrite: bool,
    pub output_scroll: usize,
    pub output_follow: bool,
    pub modal: Option<Modal>,
    pub message_scroll: usize,
    pub message_max_scroll: usize,
    pub message_page_size: usize,
    pub pending_external: Option<CommandSpec>,
    pub pending_repo: Option<PathBuf>,
    pub busy: Option<(Cancellation, Instant, String)>,
    pub loading: bool,
    pub history_loading: bool,
    pub history_exhausted: bool,
    pub history_elapsed_ms: u128,
    pub history_error: Option<String>,
    history_key: Option<HistoryKey>,
    pub preview_loading: bool,
    pub changed_only: bool,
    pub fullscreen: bool,
    pub quit: bool,
    pub dirty: bool,
    pub notice: String,
    pub options: ReadOptions,
    pub last_refresh: Instant,
    pub last_fetch: Instant,
    preview_key: Option<(PreviewTarget, bool, usize, bool)>,
}

#[derive(PartialEq, Eq)]
struct HistoryKey {
    options: ReadOptions,
    references: Vec<(String, String, bool)>,
    head: String,
}

pub fn panel_index(panel: Panel) -> usize {
    match panel {
        Panel::Status => 0,
        Panel::Files => 1,
        Panel::Branches => 2,
        Panel::Commits => 3,
        Panel::Stash => 4,
        Panel::Output => 5,
        Panel::Diff => 6,
        Panel::CommitFiles => 7,
        Panel::RemoteBranches => 8,
        Panel::Details => 9,
        Panel::HeadFiles => 10,
    }
}

impl App {
    pub fn new(repo: Repository, config: Config, mut state: State) -> Self {
        state.remember(&repo.root);
        let requested = state.commit_limit.unwrap_or(config.git.commit_limit);
        let page_size = if requested == 0 {
            config.git.commit_limit.clamp(1, MAX_HISTORY_PAGE_SIZE)
        } else {
            requested.min(MAX_HISTORY_PAGE_SIZE)
        };
        let recovered = (page_size != requested).then(|| {
            state.commit_limit = Some(page_size);
            format!("Saved history initial/page size {requested} is outside the supported range 1–{MAX_HISTORY_PAGE_SIZE}.\n\nUsing {page_size} commits initially and per page. More commits still load as you navigate.\n\nYou can change this in the graph's Loaded commits… menu (L).")
        });
        let options = ReadOptions {
            ignored: state.show_ignored,
            commit_limit: page_size,
            graph: true,
            visible_refs: state
                .visible_branches
                .get(repo.root.to_string_lossy().as_ref())
                .cloned()
                .or_else(|| Some(vec!["HEAD".into()])),
            ..ReadOptions::default()
        };
        let workers = Workers::new(repo.clone());
        workers.refresh.submit(options.clone());
        let mut app = Self {
            repo,
            config,
            state,
            snapshot: Arc::new(Snapshot::default()),
            workers,
            focus: Panel::Commits,
            side: Panel::Files,
            rows: Vec::new(),
            commit_rows: Vec::new(),
            commit_files: Vec::new(),
            commit_oid: String::new(),
            stash_rows: Vec::new(),
            stash_files: Vec::new(),
            stash_inspection_oid: None,
            stash_inspection_loading: false,
            stash_context: false,
            comparison: None,
            comparison_loading: false,
            comparison_mark: None,
            head_tree_tab: false,
            head_files: Vec::new(),
            head_rows: Vec::new(),
            head_oid: String::new(),
            head_loading: false,
            head_requested: None,
            selections: [0; 11],
            scrolls: [0; 11],
            graph_working_tree: true,
            commit_details: None,
            details_lines: vec!["Working tree / Index".into()],
            details_max_scroll: 0,
            inspection_loading: false,
            inspection_oid: None,
            searches: std::array::from_fn(|_| String::new()),
            page_sizes: [15; 11],
            search_edit: None,
            commit_search_cache: RefCell::default(),
            graph_index_cache: RefCell::default(),
            branch_rows_cache: std::array::from_fn(|_| Vec::new()),
            collapsed_branches: std::array::from_fn(|_| HashSet::new()),
            collapsed: HashSet::new(),
            marked: HashSet::new(),
            marked_commit_files: HashSet::new(),
            copied_commits: Vec::new(),
            branch_tab: 0,
            commit_tab: 0,
            preview: Preview::default(),
            preview_lines: vec!["Loading repository…".into()],
            patch: Patch::default(),
            diff_line: 0,
            diff_scroll: 0,
            diff_view_height: 15,
            horizontal_scroll: 0,
            hunk_mode: true,
            diff_context: 3,
            ignore_whitespace: false,
            show_staged: false,
            output: VecDeque::new(),
            output_pending: String::new(),
            output_overwrite: false,
            output_scroll: 0,
            output_follow: true,
            modal: None,
            message_scroll: 0,
            message_max_scroll: 0,
            message_page_size: 8,
            pending_external: None,
            pending_repo: None,
            busy: None,
            loading: true,
            history_loading: false,
            history_exhausted: true,
            history_elapsed_ms: 0,
            history_error: None,
            history_key: None,
            preview_loading: false,
            changed_only: true,
            fullscreen: false,
            quit: false,
            dirty: true,
            notice: "1 repos · 2 local · 3 remote · 4 stash · 5 graph · 6 details/output · 7 files · 0 diff".into(),
            options,
            last_refresh: Instant::now(),
            last_fetch: Instant::now(),
            preview_key: None,
        };
        if let Some(message) = recovered {
            app.error(message);
            app.save_state();
        }
        app
    }

    fn is(&self, group: &str, action: &str, defaults: &[&str], key: KeyEvent) -> bool {
        self.config.binding(group, action, defaults, key)
    }
    pub fn file_panel(&self) -> Panel {
        if self.stash_context {
            Panel::CommitFiles
        } else if self.head_tree_tab {
            Panel::HeadFiles
        } else if self.graph_working_tree && self.comparison.is_none() {
            Panel::Files
        } else {
            Panel::CommitFiles
        }
    }
    pub fn details_panel(&self) -> Panel {
        if self.state.hide_command_log {
            Panel::Details
        } else {
            Panel::Output
        }
    }
    fn show_command_output(&mut self) {
        self.state.hide_command_log = false;
        if self.focus == Panel::Details {
            self.focus = Panel::Output;
        }
    }
    fn toggle_details_tab(&mut self) {
        self.state.hide_command_log = !self.state.hide_command_log;
        if matches!(self.focus, Panel::Details | Panel::Output) {
            self.set_focus(self.details_panel());
        }
        self.save_state();
    }
    fn file_marks(&self) -> &HashSet<PathBuf> {
        if self.side == Panel::CommitFiles {
            &self.marked_commit_files
        } else {
            &self.marked
        }
    }
    fn file_marks_mut(&mut self) -> &mut HashSet<PathBuf> {
        if self.side == Panel::CommitFiles {
            &mut self.marked_commit_files
        } else {
            &mut self.marked
        }
    }
    pub fn file_is_marked(&self, path: &Path) -> bool {
        if self.stash_context {
            return false;
        }
        let marks = match self.file_panel() {
            Panel::Files => &self.marked,
            Panel::CommitFiles => &self.marked_commit_files,
            _ => return false,
        };
        marks.iter().any(|marked| path.starts_with(marked))
    }
    fn clear_file_marks(&mut self) -> bool {
        if self.side == Panel::HeadFiles || self.file_marks().is_empty() {
            return false;
        }
        self.file_marks_mut().clear();
        self.preview_key = None;
        self.request_preview(self.focus == Panel::Diff);
        true
    }
    fn mark_current_file(&mut self, toggle: bool) {
        if self.stash_context {
            return;
        }
        if let Some(path) = self.row().map(|row| row.path.clone()) {
            let marks = self.file_marks_mut();
            if !toggle || !marks.remove(&path) {
                marks.insert(path);
            }
        }
    }
    fn set_all_directories(&mut self, collapse: Option<bool>) {
        let files = match self.file_panel() {
            Panel::HeadFiles => &self.head_files,
            Panel::CommitFiles if self.stash_context => &self.stash_files,
            Panel::CommitFiles => &self.commit_files,
            _ => &self.snapshot.files,
        };
        let directories: HashSet<_> = files
            .iter()
            .flat_map(|file| file.path.ancestors().skip(1))
            .filter(|path| !path.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .collect();
        let collapse = collapse.unwrap_or_else(|| {
            directories
                .iter()
                .any(|path| !self.collapsed.contains(path))
        });
        if collapse {
            self.collapsed.extend(directories);
        } else {
            self.collapsed.retain(|path| !directories.contains(path));
        }
        self.rebuild();
        self.preview_key = None;
        self.request_preview(false);
    }
    pub fn branch_visible(&self, reference: &crate::model::Reference) -> bool {
        self.options
            .visible_refs
            .as_ref()
            .map_or(reference.current, |refs| {
                refs.contains(&reference.full_name)
                    || (reference.current && refs.iter().any(|name| name == "HEAD"))
            })
    }
    fn toggle_branch_visibility(&mut self, references: &[crate::model::Reference]) {
        if references.is_empty() {
            return;
        }
        let enable = !references
            .iter()
            .all(|reference| self.branch_visible(reference));
        let refs = self.options.visible_refs.get_or_insert_with(Vec::new);
        for reference in references {
            if enable {
                if !refs.iter().any(|name| {
                    name == &reference.full_name || (reference.current && name == "HEAD")
                }) {
                    refs.push(reference.full_name.clone());
                }
            } else {
                refs.retain(|name| {
                    name != &reference.full_name && !(reference.current && name == "HEAD")
                });
            }
        }
        self.state
            .visible_branches
            .insert(self.repo.root.to_string_lossy().into_owned(), refs.clone());
        self.save_state();
        self.commit_tab = 0;
        self.options.commit_limit = self.history_page_size();
        self.loading = false;
        self.refresh();
    }
    pub fn apply_snapshot(&mut self, snapshot: Snapshot) {
        let selected_oid = self.selected_commit().map(|commit| commit.oid.clone());
        let selected_stash_oid = self
            .stash_context
            .then(|| self.selected_stash().map(|stash| stash.oid.clone()))
            .flatten();
        let files_changed = self.snapshot.files != snapshot.files;
        self.snapshot = Arc::new(snapshot);
        self.request_head_tree();
        if let Some(oid) = selected_stash_oid {
            self.selections[4] = self
                .filtered_stashes()
                .iter()
                .position(|index| self.snapshot.stashes[*index].oid == oid)
                .unwrap_or(0);
        }
        if self.options.visible_refs.is_none() {
            self.options.visible_refs = Some(vec![
                self.snapshot
                    .references
                    .iter()
                    .find(|reference| reference.current)
                    .map(|reference| reference.full_name.clone())
                    .unwrap_or_else(|| "HEAD".into()),
            ]);
        }
        if let Some(oid) = selected_oid {
            self.selections[3] = self
                .filtered_commits()
                .iter()
                .position(|index| self.snapshot.commits[*index].oid == oid)
                .unwrap_or(0);
        }
        if self.snapshot.commits.is_empty() {
            self.graph_working_tree = true;
        }
        if files_changed {
            self.rebuild();
        } else {
            self.rebuild_branch_rows();
        }
        self.clamp_selections();
        self.sync_inspection();
        self.request_stash_files();
        self.preview_key = None;
        self.request_preview(self.focus == Panel::Diff);
        self.dirty = true;
    }
    fn clamp_selections(&mut self) {
        for panel in [
            Panel::Status,
            Panel::Files,
            Panel::CommitFiles,
            Panel::HeadFiles,
            Panel::Branches,
            Panel::RemoteBranches,
            Panel::Commits,
            Panel::Stash,
        ] {
            let index = panel_index(panel);
            self.selections[index] =
                self.selections[index].min(self.length(panel).saturating_sub(1));
        }
    }
    fn rebuild_commit_rows(&mut self) {
        self.commit_rows = tree::rows(
            &self.commit_files,
            self.config.gui.show_file_tree,
            true,
            &self.collapsed,
            self.search(Panel::CommitFiles),
            false,
        );
    }
    fn rebuild_stash_rows(&mut self) {
        self.stash_rows = tree::rows(
            &self.stash_files,
            self.config.gui.show_file_tree,
            true,
            &self.collapsed,
            self.search(Panel::CommitFiles),
            false,
        );
        self.selections[7] = self.selections[7].min(self.stash_rows.len().saturating_sub(1));
    }
    fn clear_stash_inspection(&mut self) {
        self.stash_context = false;
        self.workers.stash_files.cancel();
        self.stash_inspection_oid = None;
        self.stash_inspection_loading = false;
        self.stash_files.clear();
        self.stash_rows.clear();
    }
    fn request_stash_files(&mut self) {
        if !self.stash_context {
            return;
        }
        let Some(stash) = self.selected_stash() else {
            self.workers.stash_files.cancel();
            self.stash_inspection_oid = None;
            self.stash_inspection_loading = false;
            self.stash_files.clear();
            self.stash_rows.clear();
            self.selections[7] = 0;
            return;
        };
        let oid = stash.oid.clone();
        if self.stash_inspection_oid.as_ref() == Some(&oid) {
            return;
        }
        self.stash_inspection_oid = Some(oid.clone());
        self.stash_files.clear();
        self.stash_rows.clear();
        self.stash_inspection_loading = true;
        self.selections[7] = 0;
        self.scrolls[7] = 0;
        self.workers.stash_files.submit(oid);
    }
    pub fn file_rows(&self, panel: Panel) -> &[Row] {
        match panel {
            Panel::Files => &self.rows,
            Panel::HeadFiles => &self.head_rows,
            Panel::CommitFiles if self.stash_context => &self.stash_rows,
            _ => &self.commit_rows,
        }
    }
    pub fn stash_file_title(&self) -> String {
        self.stash_inspection_oid
            .as_ref()
            .and_then(|oid| {
                self.snapshot
                    .stashes
                    .iter()
                    .find(|stash| &stash.oid == oid)
                    .map(|stash| stash.subject.clone())
            })
            .or_else(|| {
                self.stash_inspection_oid
                    .as_ref()
                    .map(|oid| oid[..oid.len().min(8)].to_owned())
            })
            .unwrap_or_else(|| "no stash selected".into())
    }
    pub fn stash_inspection_active(&self) -> bool {
        self.stash_context
    }
    pub fn stash_inspection_loading(&self) -> bool {
        self.stash_inspection_loading
    }
    fn rebuild_head_rows(&mut self) {
        self.head_rows = tree::rows(
            &self.head_files,
            self.config.gui.show_file_tree,
            true,
            &self.collapsed,
            self.search(Panel::HeadFiles),
            false,
        );
        self.selections[10] = self.selections[10].min(self.head_rows.len().saturating_sub(1));
    }
    fn request_head_tree(&mut self) {
        if !self.head_tree_tab || self.head_requested.as_ref() == Some(&self.snapshot.head) {
            return;
        }
        self.head_requested = Some(self.snapshot.head.clone());
        self.head_oid = self.snapshot.head.clone();
        self.head_files.clear();
        self.head_rows.clear();
        self.head_loading = !self.head_oid.is_empty();
        if self.head_loading {
            self.workers.head_tree.submit(self.head_oid.clone());
        } else {
            self.workers.head_tree.cancel();
        }
    }
    pub fn toggle_file_tab(&mut self) {
        self.head_tree_tab = !self.head_tree_tab;
        self.request_head_tree();
        self.set_focus(self.file_panel());
        self.preview_key = None;
        self.request_preview(false);
    }

    fn comparison_revision(&self) -> Option<Revision> {
        if matches!(self.focus, Panel::Branches | Panel::RemoteBranches) {
            return self.selected_ref().map(|reference| Revision {
                name: reference.name.clone(),
                revision: reference.oid.clone(),
            });
        }
        if self.focus != Panel::Commits {
            if let Some(comparison) = &self.comparison {
                return Some(Revision {
                    name: comparison.request.to.name.clone(),
                    revision: comparison.to_oid.clone(),
                })
                .filter(|revision| !revision.revision.is_empty());
            }
        }
        self.selected_commit()
            .map(|commit| Revision {
                name: commit.short.clone(),
                revision: commit.oid.clone(),
            })
            .or_else(|| {
                (!self.snapshot.head.is_empty()).then(|| Revision {
                    name: "HEAD".into(),
                    revision: self.snapshot.head.clone(),
                })
            })
    }

    fn comparison_menu(&mut self) {
        let selected = self.comparison_revision();
        let mut items = Vec::new();
        if let (Some(from), Some(to)) = (&self.comparison_mark, &selected) {
            items.push(MenuItem {
                label: format!("Compare marked {} → {}", from.name, to.name),
                action: MenuAction::Compare(ComparisonRequest {
                    from: from.clone(),
                    to: to.clone(),
                    merge_base: false,
                }),
            });
        }
        if let Some(to) = selected {
            items.push(MenuItem {
                label: format!("Mark {} as comparison start (A)", to.name),
                action: MenuAction::CompareMark(to.clone()),
            });
            if !self.snapshot.head.is_empty() {
                items.push(MenuItem {
                    label: format!("Compare HEAD → {}", to.name),
                    action: MenuAction::Compare(ComparisonRequest {
                        from: Revision {
                            name: "HEAD".into(),
                            revision: self.snapshot.head.clone(),
                        },
                        to: to.clone(),
                        merge_base: false,
                    }),
                });
            }
            items.push(MenuItem {
                label: format!(
                    "Compare {} with base branch… (common ancestor → selected)",
                    to.name
                ),
                action: MenuAction::Input(Prompt::CompareBase(to)),
            });
        }
        items.push(MenuItem {
            label: "Compare two commits / branches / tags by name…".into(),
            action: MenuAction::Input(Prompt::CompareFrom),
        });
        if let Some(info) = &self.comparison {
            if !self.comparison_loading {
                items.push(MenuItem {
                    label: "Swap comparison direction".into(),
                    action: MenuAction::Compare(ComparisonRequest {
                        from: Revision {
                            name: info.request.to.name.clone(),
                            revision: info.to_oid.clone(),
                        },
                        to: Revision {
                            name: info.request.from.name.clone(),
                            revision: info.from_oid.clone(),
                        },
                        merge_base: info.request.merge_base,
                    }),
                });
            }
            items.push(MenuItem {
                label: "Close comparison".into(),
                action: MenuAction::CompareClose,
            });
        }
        if self.comparison_mark.is_some() {
            items.push(MenuItem {
                label: "Clear marked comparison start".into(),
                action: MenuAction::CompareUnmark,
            });
        }
        self.menu(
            "Compare revisions · A → B shows changes needed to reach B",
            items,
        );
    }

    pub fn start_comparison(&mut self, request: ComparisonRequest) {
        self.marked_commit_files.clear();
        self.clear_stash_inspection();
        self.workers.commit_files.cancel();
        self.workers.preview.cancel();
        self.inspection_oid = None;
        self.inspection_loading = false;
        self.commit_details = None;
        self.commit_oid.clear();
        self.commit_files.clear();
        self.commit_rows.clear();
        self.searches[7].clear();
        self.selections[7] = 0;
        self.scrolls[7] = 0;
        self.scrolls[9] = 0;
        self.head_tree_tab = false;
        self.fullscreen = false;
        self.comparison = Some(ComparisonInfo {
            request: request.clone(),
            from_oid: String::new(),
            to_oid: String::new(),
            merge_base_oid: None,
            summary: "Loading comparison…".into(),
        });
        self.comparison_loading = true;
        self.workers.comparison.submit(request);
        self.update_details();
        self.set_focus(Panel::CommitFiles);
    }

    pub fn close_comparison(&mut self) {
        if self.comparison.take().is_none() {
            return;
        }
        self.workers.comparison.cancel();
        self.workers.preview.cancel();
        self.comparison_loading = false;
        self.marked_commit_files.clear();
        self.inspection_oid = None;
        self.commit_files.clear();
        self.commit_rows.clear();
        self.head_tree_tab = false;
        self.fullscreen = false;
        self.focus = Panel::Commits;
        self.sync_inspection();
        self.preview_key = None;
        self.request_preview(false);
        self.dirty = true;
    }

    fn update_details(&mut self) {
        self.details_lines = if let Some(info) = &self.comparison {
            let mut lines = vec![
                format!("Comparison: {}", info.title()),
                String::new(),
                format!("From: {}", info.request.from.name),
                format!("To: {}", info.request.to.name),
            ];
            if !self.comparison_loading {
                lines.push(format!("From commit: {}", info.from_oid));
                lines.push(format!("To commit: {}", info.to_oid));
                if let Some(base) = &info.merge_base_oid {
                    lines.push(format!("Common ancestor: {base}"));
                }
                lines.push(format!("{} changed files", self.commit_files.len()));
            }
            lines.extend([
                String::new(),
                info.summary.clone(),
                String::new(),
                "7 files · 0 diff · B comparison options · Esc close".into(),
            ]);
            lines
        } else if self.graph_working_tree {
            vec![
                "Working tree / Index".into(),
                String::new(),
                format!("Branch: {}", self.snapshot.branch),
                format!(
                    "{} changed files · {} staged",
                    self.snapshot
                        .files
                        .iter()
                        .filter(|file| file.changed())
                        .count(),
                    self.snapshot
                        .files
                        .iter()
                        .filter(|file| file.staged())
                        .count()
                ),
                String::new(),
                "c commit · C editor · A amend".into(),
                "7 inspect files · 0 inspect diff".into(),
            ]
        } else if let Some(commit) = self.selected_commit() {
            let reference_lines = self.commit_reference_lines(&commit.oid);
            if let Some(details) = self
                .commit_details
                .as_ref()
                .filter(|details| details.oid == commit.oid)
            {
                let mut lines: Vec<String> = details.message.lines().map(str::to_owned).collect();
                lines.extend([
                    String::new(),
                    format!("Commit {}", details.oid),
                    format!("Author: {} <{}>", details.author, details.author_email),
                    format!(
                        "Committer: {} <{}>",
                        details.committer, details.committer_email
                    ),
                    format!("Date: {}", details.committed_at),
                    format!("Parents: {}", details.parents.join(" ")),
                ]);
                lines.extend(reference_lines);
                lines.push(format!("{} changed files", self.commit_files.len()));
                lines
            } else {
                let mut lines = vec![
                    format!("Commit {}", commit.oid),
                    format!("Author: {}", commit.author),
                    format!("Date: {}", commit.date),
                    String::new(),
                    commit.subject.clone(),
                    String::new(),
                ];
                lines.extend(reference_lines);
                lines.push("Loading full message and changed files…".into());
                lines
            }
        } else {
            vec![
                "No commit selected".into(),
                "Space on a branch toggles its graph visibility.".into(),
            ]
        };
    }
    fn commit_reference_lines(&self, oid: &str) -> Vec<String> {
        let names = |kind| {
            self.snapshot
                .references
                .iter()
                .filter(|reference| reference.oid == oid && reference.kind == kind)
                .map(|reference| reference.name.as_str())
                .collect::<Vec<_>>()
        };
        [
            (RefKind::Branch, "Local branches"),
            (RefKind::Remote, "Remote branches"),
            (RefKind::Tag, "Tags"),
        ]
        .into_iter()
        .map(|(kind, label)| {
            let names = names(kind);
            format!(
                "{label}: {}",
                if names.is_empty() {
                    "—".into()
                } else {
                    names.join(", ")
                }
            )
        })
        .collect()
    }
    fn sync_inspection(&mut self) {
        if self.comparison.is_some() {
            if self.focus != Panel::Stash {
                self.side = if self.head_tree_tab {
                    Panel::HeadFiles
                } else {
                    Panel::CommitFiles
                };
            }
            self.update_details();
            return;
        }
        if !self.graph_working_tree && self.selected_commit().is_none() {
            self.graph_working_tree = true;
        }
        if self.graph_working_tree {
            if self.inspection_oid.take().is_some() {
                self.marked_commit_files.clear();
                self.workers.commit_files.cancel();
                self.commit_files.clear();
                self.commit_rows.clear();
                self.commit_oid.clear();
            }
            self.commit_details = None;
            self.inspection_loading = false;
            if self.focus == Panel::CommitFiles && !self.head_tree_tab && !self.stash_context {
                self.focus = Panel::Files;
            }
            if self.focus != Panel::Stash && !self.head_tree_tab && !self.stash_context {
                self.side = Panel::Files;
            }
        } else if let Some(oid) = self.selected_commit().map(|commit| commit.oid.clone()) {
            if self.inspection_oid.as_ref() != Some(&oid) {
                self.marked_commit_files.clear();
                self.inspection_oid = Some(oid.clone());
                self.commit_oid = oid.clone();
                self.commit_files.clear();
                self.commit_rows.clear();
                self.commit_details = None;
                self.inspection_loading = true;
                self.selections[7] = 0;
                self.scrolls[7] = 0;
                self.scrolls[9] = 0;
                self.workers.commit_files.submit(oid);
            }
            if self.focus != Panel::Stash && !self.head_tree_tab && !self.stash_context {
                self.side = Panel::CommitFiles;
            }
        } else {
            self.workers.commit_files.cancel();
            self.inspection_loading = false;
            self.commit_files.clear();
            self.commit_rows.clear();
            self.commit_details = None;
            self.inspection_oid = None;
        }
        if self.head_tree_tab && self.focus != Panel::Stash && !self.stash_context {
            self.side = Panel::HeadFiles;
        }
        self.update_details();
    }
    /// The worktree belongs to HEAD, even when another visible branch is newer.
    pub fn working_tree_graph_index(&self) -> usize {
        self.filtered_commits();
        self.commit_search_cache.borrow().working_tree
    }
    pub fn graph_index(&self) -> Arc<GraphIndex> {
        let order = self.filtered_commits();
        let searching = !self.search(Panel::Commits).is_empty();
        let mut cached = self.graph_index_cache.borrow_mut();
        if let Some(index) = cached
            .as_ref()
            .filter(|index| index.matches(&order, searching))
        {
            return index.clone();
        }
        let index = Arc::new(GraphIndex::new(order, &self.snapshot.commits, searching));
        *cached = Some(index.clone());
        index
    }
    pub fn commit_graph_row(&self, filtered_index: usize) -> usize {
        filtered_index + usize::from(filtered_index >= self.working_tree_graph_index())
    }
    pub fn selected_graph_row(&self) -> usize {
        if self.graph_working_tree {
            self.working_tree_graph_index()
        } else {
            self.commit_graph_row(self.selected(Panel::Commits))
        }
    }
    pub fn select_graph_row(&mut self, logical_index: usize) {
        self.clear_stash_inspection();
        self.close_comparison();
        self.focus = Panel::Commits;
        let worktree_index = self.working_tree_graph_index();
        self.graph_working_tree = logical_index == worktree_index;
        self.selections[3] = logical_index
            .saturating_sub(usize::from(logical_index > worktree_index))
            .min(self.length(Panel::Commits).saturating_sub(1));
        self.sync_inspection();
        self.preview_key = None;
        self.diff_line = 0;
        self.diff_scroll = 0;
        self.request_preview(false);
        self.dirty = true;
        if !self.graph_working_tree
            && self.selected(Panel::Commits).saturating_add(50) >= self.length(Panel::Commits)
        {
            self.load_more_history();
        }
    }
    pub fn select_list_row(&mut self, panel: Panel, index: usize) {
        if panel == Panel::Commits {
            self.select_graph_row(index);
            return;
        }
        self.set_focus(panel);
        self.selections[panel_index(panel)] = index.min(self.length(panel).saturating_sub(1));
        if panel == Panel::Stash {
            self.request_stash_files();
        }
        if !matches!(
            panel,
            Panel::Status | Panel::Branches | Panel::RemoteBranches | Panel::Details
        ) {
            self.request_preview(false);
        }
        self.dirty = true;
    }
    pub fn selected(&self, panel: Panel) -> usize {
        self.selections[panel_index(panel)]
    }
    pub fn search(&self, panel: Panel) -> &str {
        &self.searches[panel_index(panel)]
    }
    pub fn row(&self) -> Option<&Row> {
        if self.side == Panel::HeadFiles {
            self.head_rows.get(self.selected(Panel::HeadFiles))
        } else if self.side == Panel::CommitFiles {
            self.file_rows(Panel::CommitFiles)
                .get(self.selected(Panel::CommitFiles))
        } else {
            self.rows.get(self.selected(Panel::Files))
        }
    }
    pub fn filtered_refs(&self) -> Vec<usize> {
        self.refs_for(self.focus == Panel::RemoteBranches)
    }
    pub fn branch_rows(&self, remote: bool) -> &[BranchRow] {
        &self.branch_rows_cache[usize::from(remote)]
    }
    pub fn selected_branch_row(&self) -> Option<&BranchRow> {
        if !matches!(self.focus, Panel::Branches | Panel::RemoteBranches)
            || (self.focus == Panel::Branches && self.branch_tab == 3)
        {
            return None;
        }
        self.branch_rows(self.focus == Panel::RemoteBranches)
            .get(self.selected(self.focus))
    }
    pub fn rebuild_branch_rows(&mut self) {
        for remote in [false, true] {
            let cache = usize::from(remote);
            if !remote && self.branch_tab == 3 {
                self.branch_rows_cache[cache].clear();
                continue;
            }
            let panel = if remote {
                Panel::RemoteBranches
            } else {
                Panel::Branches
            };
            let selection = panel_index(panel);
            let old = self.branch_rows_cache[cache]
                .get(self.selections[selection])
                .map(|row| (row.path.clone(), row.is_group()));
            let delimiter = if !remote && self.branch_tab != 0 {
                ""
            } else {
                &self.config.gui.branch_group_delimiter
            };
            let indices = if !remote && self.branch_tab == 3 {
                vec![]
            } else {
                self.refs_for(remote)
            };
            let rows = branches::rows(
                &self.snapshot.references,
                &indices,
                delimiter,
                &self.collapsed_branches[cache],
                !self.search(panel).is_empty(),
            );
            if let Some((path, group)) = old {
                if let Some(index) = rows
                    .iter()
                    .position(|row| row.path == path && row.is_group() == group)
                    .or_else(|| {
                        rows.iter().rposition(|row| {
                            row.is_group()
                                && !delimiter.is_empty()
                                && (row.path == path
                                    || path.starts_with(&format!("{}{delimiter}", row.path)))
                        })
                    })
                {
                    self.selections[selection] = index;
                }
            }
            self.selections[selection] =
                self.selections[selection].min(rows.len().saturating_sub(1));
            self.branch_rows_cache[cache] = rows;
        }
    }
    fn navigate_branch_group(&mut self, expand: bool) -> bool {
        let Some(row) = self.selected_branch_row().cloned() else {
            return false;
        };
        let remote = self.focus == Panel::RemoteBranches;
        let cache = usize::from(remote);
        if expand && row.is_group() {
            if !self.collapsed_branches[cache].remove(&row.path) {
                let next = self.selected(self.focus) + 1;
                if self
                    .branch_rows(remote)
                    .get(next)
                    .is_some_and(|child| child.depth > row.depth)
                {
                    self.selections[panel_index(self.focus)] = next;
                }
            }
        } else if !expand && row.is_group() && !self.collapsed_branches[cache].contains(&row.path) {
            self.collapsed_branches[cache].insert(row.path);
        } else if !expand {
            let index = self.selected(self.focus);
            if let Some(parent) = self.branch_rows(remote)[..index]
                .iter()
                .rposition(|parent| parent.depth < row.depth)
            {
                self.selections[panel_index(self.focus)] = parent;
            } else {
                return false;
            }
        } else {
            return false;
        }
        self.rebuild_branch_rows();
        self.dirty = true;
        true
    }
    pub fn refs_for(&self, remote: bool) -> Vec<usize> {
        let kind = match (remote, self.branch_tab) {
            (true, _) => RefKind::Remote,
            (false, 2) => RefKind::Tag,
            _ => RefKind::Branch,
        };
        let panel = if remote {
            Panel::RemoteBranches
        } else {
            Panel::Branches
        };
        let query = crate::fuzzy::Query::new(self.search(panel));
        self.snapshot
            .references
            .iter()
            .enumerate()
            .filter(|(_, reference)| reference.kind == kind && query.is_match(&reference.name))
            .map(|(index, _)| index)
            .collect()
    }
    pub fn filtered_commits(&self) -> CommitOrder {
        let mut cache = self.commit_search_cache.borrow_mut();
        let query = self.search(Panel::Commits);
        let changed = cache.query != query || !Arc::ptr_eq(&cache.commits, &self.snapshot.commits);
        if changed {
            if query.is_empty() {
                cache.matches = CommitOrder::new((0..self.snapshot.commits.len()).collect());
            } else {
                let compiled = crate::fuzzy::Query::new(query);
                let mut matches: Vec<_> = self
                    .snapshot
                    .commits
                    .iter()
                    .enumerate()
                    .filter_map(|(index, commit)| {
                        compiled
                            .score(&format!(
                                "{} {} {} {} {}",
                                commit.oid,
                                commit.subject,
                                commit.author,
                                commit.committer,
                                commit.decorations
                            ))
                            .map(|score| {
                                (
                                    index,
                                    score
                                        + compiled
                                            .score(&commit.subject)
                                            .map_or(0, |subject| 128 + subject.max(0)),
                                )
                            })
                    })
                    .collect();
                matches.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
                cache.matches =
                    CommitOrder::new(matches.into_iter().map(|(index, _)| index).collect());
            }
            cache.commits = self.snapshot.commits.clone();
            cache.query = query.to_owned();
        }
        if changed || cache.head != self.snapshot.head {
            cache.working_tree = cache
                .matches
                .iter()
                .position(|index| self.snapshot.commits[*index].oid == self.snapshot.head)
                .unwrap_or(0);
            cache.head = self.snapshot.head.clone();
        }
        cache.matches.clone()
    }
    pub fn filtered_stashes(&self) -> Vec<usize> {
        self.snapshot
            .stashes
            .iter()
            .enumerate()
            .filter(|(_, stash)| fuzzy_match(&stash.subject, self.search(Panel::Stash)))
            .map(|(index, _)| index)
            .collect()
    }
    pub fn selected_commit(&self) -> Option<&crate::model::Commit> {
        if self.graph_working_tree {
            return None;
        }
        self.filtered_commits()
            .get(self.selected(Panel::Commits))
            .and_then(|index| self.snapshot.commits.get(*index))
    }
    pub fn selected_ref(&self) -> Option<&crate::model::Reference> {
        let remote = self.focus == Panel::RemoteBranches;
        self.branch_rows(remote)
            .get(self.selected(if remote {
                Panel::RemoteBranches
            } else {
                Panel::Branches
            }))
            .and_then(|row| row.reference)
            .and_then(|index| self.snapshot.references.get(index))
    }
    pub fn selected_stash(&self) -> Option<&crate::model::Stash> {
        self.filtered_stashes()
            .get(self.selected(Panel::Stash))
            .and_then(|index| self.snapshot.stashes.get(*index))
    }
    fn length(&self, panel: Panel) -> usize {
        match panel {
            Panel::Files => self.rows.len(),
            Panel::CommitFiles => self.file_rows(panel).len(),
            Panel::HeadFiles => self.head_rows.len(),
            Panel::Branches if self.branch_tab == 3 => self.snapshot.worktrees.len(),
            Panel::Branches => self.branch_rows(false).len(),
            Panel::RemoteBranches => self.branch_rows(true).len(),
            Panel::Commits => self.filtered_commits().len(),
            Panel::Stash => self.filtered_stashes().len(),
            Panel::Output => self.output.len(),
            Panel::Diff => self.preview_lines.len(),
            Panel::Status => self.state.recent_repos.len(),
            Panel::Details => self.details_lines.len(),
        }
    }
    pub fn rebuild(&mut self) {
        self.rebuild_branch_rows();
        let selected = self
            .rows
            .get(self.selected(Panel::Files))
            .map(|row| row.path.clone());
        self.rows = tree::rows(
            &self.snapshot.files,
            self.config.gui.show_file_tree,
            self.config.gui.show_unchanged_files,
            &self.collapsed,
            self.search(Panel::Files),
            self.changed_only,
        );
        if let Some(path) = selected {
            if let Some(index) = self.rows.iter().position(|row| row.path == path) {
                self.selections[1] = index;
            }
        }
        self.rebuild_commit_rows();
        self.rebuild_stash_rows();
        self.rebuild_head_rows();
        self.clamp_selections();
        self.dirty = true;
    }

    pub fn refresh(&mut self) {
        if self.loading {
            return;
        }
        self.options.ignored = self.state.show_ignored;
        self.options.reflog = self.commit_tab == 1;
        self.workers.refresh.submit(self.options.clone());
        self.loading = true;
        self.last_refresh = Instant::now();
    }

    fn request_history(&mut self) {
        let mut options = self.options.clone();
        options.ignored = false;
        options.reflog = self.commit_tab == 1;
        let key = HistoryKey {
            options,
            references: self
                .snapshot
                .references
                .iter()
                .map(|reference| {
                    (
                        reference.full_name.clone(),
                        reference.oid.clone(),
                        reference.current,
                    )
                })
                .collect(),
            head: self.snapshot.head.clone(),
        };
        if self.history_key.as_ref() == Some(&key) {
            return;
        }
        self.workers.history.submit(key.options.clone());
        self.history_key = Some(key);
        self.history_error = None;
        self.history_loading = true;
        self.dirty = true;
    }

    fn load_more_history(&mut self) {
        if self.history_loading || self.history_exhausted || self.history_error.is_some() {
            return;
        }
        self.options.commit_limit = self
            .options
            .commit_limit
            .saturating_add(self.history_page_size());
        self.request_history();
    }

    pub fn history_page_size(&self) -> usize {
        self.state
            .commit_limit
            .filter(|limit| *limit > 0)
            .unwrap_or(self.config.git.commit_limit)
            .clamp(1, MAX_HISTORY_PAGE_SIZE)
    }

    fn set_history_limit(&mut self, limit: usize) -> Result<()> {
        if !(1..=MAX_HISTORY_PAGE_SIZE).contains(&limit) {
            bail!(
                "Enter a positive number between 1 and {MAX_HISTORY_PAGE_SIZE} commits.\nThis sets the initial/page size; more commits load as you navigate."
            );
        }
        self.state.commit_limit = Some(limit);
        self.save_state();
        self.options.commit_limit = limit;
        // Choosing a size is an explicit retry, including choosing the same
        // size after a failed read. Routine status refreshes never retry it.
        if self.history_error.is_some() {
            self.history_key = None;
        }
        self.request_history();
        self.notice = format!(
            "History initial/page size: {limit}; older commits load automatically near the end"
        );
        Ok(())
    }

    fn history_limit_menu(&mut self) {
        let size = self.history_page_size();
        let mut items = [100, 300, 1000, 5000, 10000]
            .into_iter()
            .map(|limit| MenuItem {
                label: format!(
                    "{} {limit} commits initially / per page",
                    if size == limit { "●" } else { "○" }
                ),
                action: MenuAction::HistoryLimit(limit),
            })
            .collect::<Vec<_>>();
        items.push(MenuItem {
            label: "Custom initial/page size…".into(),
            action: MenuAction::Input(Prompt::HistoryLimit),
        });
        if self.history_error.is_some() {
            items.push(MenuItem {
                label: "Retry failed history load".into(),
                action: MenuAction::HistoryRetry,
            });
        } else if !self.history_exhausted && !self.history_loading {
            items.push(MenuItem {
                label: format!("Load more now (+{size}; automatic paging stays on)"),
                action: MenuAction::HistoryMore,
            });
        }
        self.menu(
            format!(
                "History loading · loaded {} · target {} · initial/page {size}",
                self.snapshot.commits.len(),
                self.options.commit_limit
            ),
            items,
        );
    }

    fn target(&self) -> Option<PreviewTarget> {
        if self.side == Panel::CommitFiles {
            if let Some(info) = &self.comparison {
                if self.comparison_loading {
                    return None;
                }
                let row = self.row()?;
                let file = row
                    .file_index
                    .and_then(|index| self.commit_files.get(index));
                return Some(PreviewTarget::Comparison {
                    from: info.diff_base().into(),
                    to: info.to_oid.clone(),
                    path: row.path.clone(),
                    old_path: file.and_then(|file| file.old_path.clone()),
                });
            }
            if let Some(oid) = &self.stash_inspection_oid {
                let row = self.row()?;
                if row.directory {
                    return None;
                }
                return Some(PreviewTarget::StashFile {
                    oid: oid.clone(),
                    path: row.path.clone(),
                });
            }
            if !self.marked_commit_files.is_empty() {
                let paths = self.selected_file_paths();
                if !paths.is_empty() {
                    return Some(PreviewTarget::CommitFiles {
                        oid: self.commit_oid.clone(),
                        paths,
                    });
                }
            }
        }
        match self.side {
            Panel::HeadFiles => {
                let row = self.row()?;
                if row.directory || self.head_oid.is_empty() {
                    return None;
                }
                Some(PreviewTarget::TreeFile {
                    path: row.path.clone(),
                    oid: self.head_oid.clone(),
                })
            }
            Panel::Files | Panel::CommitFiles => {
                let row = self.row()?;
                let files = if self.side == Panel::CommitFiles {
                    &self.commit_files
                } else {
                    &self.snapshot.files
                };
                let file = row.file_index.and_then(|index| files.get(index));
                Some(PreviewTarget::File {
                    path: row.path.clone(),
                    old_path: file.and_then(|file| file.old_path.clone()),
                    untracked: file.is_some_and(FileEntry::untracked),
                    ignored: row.ignored,
                    staged: self.side != Panel::CommitFiles
                        && (self.show_staged
                            || file.is_some_and(|file| file.staged() && !file.unstaged())),
                    commit: (self.side == Panel::CommitFiles).then(|| self.commit_oid.clone()),
                })
            }
            Panel::Commits => self
                .selected_commit()
                .map(|commit| PreviewTarget::Commit(commit.oid.clone())),
            Panel::Branches if self.branch_tab != 3 => self
                .selected_ref()
                .map(|reference| PreviewTarget::Reference(reference.full_name.clone())),
            Panel::Stash => self
                .selected_stash()
                .map(|stash| PreviewTarget::Stash(stash.reference.clone())),
            _ => Some(PreviewTarget::Status),
        }
    }

    pub fn request_preview(&mut self, explicit: bool) {
        let Some(target) = self.target() else {
            self.workers.preview.cancel();
            self.preview_loading = false;
            self.preview_key = None;
            self.preview_lines = vec![
                if self.comparison_loading {
                    "Loading comparison…"
                } else if self.comparison.is_some() && self.commit_files.is_empty() {
                    "No differences between these revisions."
                } else {
                    "No selection"
                }
                .into(),
            ];
            self.preview = Preview::default();
            self.patch = Patch::default();
            self.dirty = true;
            return;
        };
        let explicit = explicit || matches!(target, PreviewTarget::Comparison { .. });
        if !explicit
            && !self.state.diff_preview_enabled
            && matches!(
                self.side,
                Panel::Files | Panel::CommitFiles | Panel::HeadFiles
            )
        {
            self.workers.preview.cancel();
            self.preview_loading = false;
            self.preview_key = None;
            self.preview = Preview::default();
            self.patch = Patch::default();
            self.preview_lines = vec![
                "Automatic previews are off.".into(),
                "Press 0 or Enter to open this diff; ~ turns automatic previews on.".into(),
            ];
            self.dirty = true;
            return;
        }
        let key = (
            target.clone(),
            explicit,
            self.diff_context,
            self.ignore_whitespace,
        );
        if self.preview_key.as_ref() == Some(&key) {
            return;
        }
        self.preview_key = Some(key);
        self.preview_loading = true;
        self.workers.preview.submit(PreviewRequest {
            target,
            explicit,
            settings: self.config.gui.clone(),
            context: self.diff_context,
            ignore_whitespace: self.ignore_whitespace,
        });
        self.dirty = true;
    }

    pub fn poll_workers(&mut self) {
        while let Ok(chunk) = self.workers.output.try_recv() {
            self.append_output(&String::from_utf8_lossy(&chunk));
        }
        while let Ok(event) = self.workers.events.try_recv() {
            match event {
                WorkerEvent::References(id, references, head)
                    if id == self.workers.refresh.current() =>
                {
                    let snapshot = Arc::make_mut(&mut self.snapshot);
                    let current = references.iter().find(|reference| reference.current);
                    snapshot.ahead = current.and_then(|reference| reference.ahead);
                    snapshot.behind = current.and_then(|reference| reference.behind);
                    snapshot.upstream = current
                        .map(|reference| reference.upstream.clone())
                        .unwrap_or_default();
                    snapshot.references = references;
                    snapshot.head = head;
                    self.request_head_tree();
                    self.rebuild_branch_rows();
                    self.request_history();
                }
                WorkerEvent::Snapshot(id, result) if id == self.workers.refresh.current() => {
                    self.loading = false;
                    match result {
                        Ok(mut snapshot) => {
                            snapshot.commits = self.snapshot.commits.clone();
                            self.apply_snapshot(snapshot);
                            self.request_history();
                        }
                        Err(error) => self.error(error),
                    }
                }
                WorkerEvent::History(id, result) if id == self.workers.history.current() => {
                    self.history_loading = false;
                    match result {
                        Ok(page) => {
                            let selected_position = self.selected(Panel::Commits);
                            let selected_oid =
                                self.selected_commit().map(|commit| commit.oid.clone());
                            self.history_exhausted = page.exhausted;
                            self.history_elapsed_ms = page.elapsed_ms;
                            Arc::make_mut(&mut self.snapshot).commits = Arc::new(page.commits);
                            if let Some(oid) = selected_oid {
                                self.selections[3] = self
                                    .filtered_commits()
                                    .iter()
                                    .position(|index| self.snapshot.commits[*index].oid == oid)
                                    .unwrap_or(selected_position);
                            }
                            if self.snapshot.commits.is_empty() {
                                self.graph_working_tree = true;
                            }
                            self.clamp_selections();
                            self.sync_inspection();
                            self.preview_key = None;
                            self.request_preview(self.focus == Panel::Diff);
                        }
                        Err(error) => {
                            // Keep the failed key: references/status refresh
                            // events must not reopen this error every few seconds.
                            self.history_error = Some(error.clone());
                            let message = format!(
                                "History load failed: {error}\n\nPreviously loaded commits were kept. Automatic retries are paused.\nOpen graph actions → Loaded commits… to choose a smaller page or retry."
                            );
                            self.append_output(&format!("\n✗ {message}\n"));
                            self.show_command_output();
                            self.notice =
                                "History loading paused after an error · L opens loading options"
                                    .into();
                            if self.modal.is_none() && self.search_edit.is_none() {
                                self.error(message);
                            }
                        }
                    }
                }
                WorkerEvent::Comparison(id, result)
                    if id == self.workers.comparison.current() && self.comparison.is_some() =>
                {
                    self.comparison_loading = false;
                    match result {
                        Ok(result) => {
                            self.comparison = Some(result.info);
                            self.commit_files = result.files;
                            self.rebuild_commit_rows();
                            self.update_details();
                            self.preview_key = None;
                            self.request_preview(false);
                        }
                        Err(error) => {
                            self.close_comparison();
                            self.error(format!("Comparison failed: {error}"));
                        }
                    }
                }
                WorkerEvent::Preview(id, result) if id == self.workers.preview.current() => {
                    self.preview_loading = false;
                    match result {
                        Ok(preview) => {
                            self.patch = Patch::parse(&preview.patch);
                            self.preview_lines = preview.text.lines().map(str::to_owned).collect();
                            self.diff_line = self
                                .diff_line
                                .min(self.preview_lines.len().saturating_sub(1));
                            self.diff_scroll = self.diff_scroll.min(self.diff_line);
                            if self.focus == Panel::Diff && self.diff_line == 0 {
                                if let Some(hunk) = self.patch.hunks.first() {
                                    self.diff_line = hunk.lines.start;
                                }
                            }
                            self.preview = preview;
                        }
                        Err(error) => {
                            self.preview_lines = vec![error];
                            self.patch = Patch::default();
                            self.preview = Preview::default();
                        }
                    }
                }
                WorkerEvent::CommitFiles(id, oid, result)
                    if id == self.workers.commit_files.current()
                        && self.inspection_oid.as_ref() == Some(&oid) =>
                {
                    match result {
                        Ok(inspection) => {
                            self.inspection_loading = false;
                            self.commit_files = inspection.files;
                            self.commit_details = Some(inspection.details);
                            self.commit_oid = oid;
                            self.selections[7] = 0;
                            self.rebuild_commit_rows();
                            self.update_details();
                            self.preview_key = None;
                            self.request_preview(self.focus == Panel::Diff);
                        }
                        Err(error) => {
                            self.inspection_loading = false;
                            self.error(error);
                        }
                    }
                }
                WorkerEvent::StashFiles(id, oid, result)
                    if id == self.workers.stash_files.current()
                        && self.stash_inspection_oid.as_ref() == Some(&oid) =>
                {
                    self.stash_inspection_loading = false;
                    match result {
                        Ok(files) => {
                            self.stash_files = files;
                            self.rebuild_stash_rows();
                            self.preview_key = None;
                            self.request_preview(self.focus == Panel::Diff);
                        }
                        Err(error) => {
                            self.error(format!("Loading stashed files: {error}"));
                        }
                    }
                }
                WorkerEvent::HeadTree(id, oid, result)
                    if id == self.workers.head_tree.current()
                        && self.head_requested.as_ref() == Some(&oid) =>
                {
                    self.head_loading = false;
                    match result {
                        Ok(files) => {
                            let selected = self
                                .head_rows
                                .get(self.selections[10])
                                .map(|row| row.path.clone());
                            self.head_files = files;
                            self.head_oid = oid;
                            self.rebuild_head_rows();
                            if let Some(index) = selected.and_then(|path| {
                                self.head_rows.iter().position(|row| row.path == path)
                            }) {
                                self.selections[10] = index;
                            }
                            if self.side == Panel::HeadFiles {
                                self.preview_key = None;
                                self.request_preview(self.focus == Panel::Diff);
                            }
                        }
                        Err(error) => {
                            self.head_requested = None;
                            self.error(error);
                        }
                    }
                }
                WorkerEvent::CommandDone(result) => {
                    while let Ok(chunk) = self.workers.output.try_recv() {
                        self.append_output(&String::from_utf8_lossy(&chunk));
                    }
                    self.flush_output();
                    self.busy = None;
                    match result {
                        Ok(_) => {
                            self.notice = "Command completed".into();
                            self.append_output("\n✓ Command completed\n");
                        }
                        Err(error) => {
                            self.notice = error.clone();
                            self.append_output(&format!("\n✗ {error}\n"));
                        }
                    }
                    self.loading = false;
                    self.refresh();
                }
                _ => {}
            }
            self.dirty = true;
        }
    }

    fn append_output(&mut self, text: &str) {
        for character in text.chars() {
            match character {
                '\r' => self.output_overwrite = true,
                '\n' => {
                    self.output
                        .push_back(strip_ansi(&std::mem::take(&mut self.output_pending)));
                    self.output_overwrite = false;
                }
                _ => {
                    if self.output_overwrite {
                        self.output_pending.clear();
                        self.output_overwrite = false;
                    }
                    self.output_pending.push(character);
                    if self.output_pending.len() > 64 * 1024 {
                        self.output
                            .push_back(strip_ansi(&std::mem::take(&mut self.output_pending)));
                    }
                }
            }
        }
        while self.output.len() > self.config.gui.command_log_size {
            self.output.pop_front();
        }
        self.dirty = true;
    }
    fn flush_output(&mut self) {
        if !self.output_pending.is_empty() {
            self.output
                .push_back(strip_ansi(&std::mem::take(&mut self.output_pending)));
        }
        self.output_overwrite = false;
    }
    pub fn output_tail(&self) -> &str {
        &self.output_pending
    }
    pub fn run_operation(&mut self, operation: Operation) {
        if self.warn_if_busy(&operation.label) {
            return;
        }
        self.append_output(&format!("\n$ {}\n", operation.label));
        self.show_command_output();
        self.output_follow = true;
        let cancel = Cancellation::default();
        let label = operation.label.clone();
        match self.workers.commands.send(CommandRequest {
            operation,
            cancel: cancel.clone(),
        }) {
            Ok(()) => self.busy = Some((cancel, Instant::now(), label)),
            Err(error) => self.error(error.to_string()),
        }
        self.dirty = true;
    }
    pub fn warn_if_busy(&mut self, requested: &str) -> bool {
        let Some((_, _, running)) = &self.busy else {
            return false;
        };
        let running = running.clone();
        self.notice = format!("Another command is still running: {running}");
        self.append_output(&format!(
            "\n! Not started: {requested}\n  Another command is still running: {running}\n"
        ));
        self.modal = Some(Modal::Busy {
            running,
            requested: requested.to_owned(),
        });
        self.dirty = true;
        true
    }
    pub fn error(&mut self, message: String) {
        self.notice = message.clone();
        self.message_scroll = 0;
        self.message_max_scroll = 0;
        self.modal = Some(Modal::Message(message));
        self.dirty = true;
    }
    fn save_state(&mut self) {
        if let Err(error) = self.state.save(&self.config.directory) {
            self.error(format!("Saving preferences: {error}"));
        }
    }
    pub fn set_focus(&mut self, panel: Panel) {
        if matches!(
            panel,
            Panel::Status
                | Panel::Files
                | Panel::HeadFiles
                | Panel::Branches
                | Panel::RemoteBranches
                | Panel::Commits
        ) {
            self.clear_stash_inspection();
        }
        if matches!(panel, Panel::Commits | Panel::Stash) {
            self.close_comparison();
        }
        if self
            .search_edit
            .as_ref()
            .is_some_and(|edit| edit.panel != panel)
        {
            self.search_edit = None;
        }
        self.focus = panel;
        if matches!(panel, Panel::Files | Panel::CommitFiles | Panel::HeadFiles) {
            self.head_tree_tab = panel == Panel::HeadFiles;
            self.request_head_tree();
            self.side = panel;
            self.diff_line = 0;
            self.diff_scroll = 0;
            self.show_staged = false;
        }
        if panel == Panel::Commits {
            self.sync_inspection();
        }
        if panel == Panel::Stash {
            self.stash_context = true;
            self.head_tree_tab = false;
            self.side = Panel::Stash;
            self.request_stash_files();
        }
        if panel == Panel::Output {
            self.state.hide_command_log = false;
        } else if panel == Panel::Details {
            self.state.hide_command_log = true;
        }
        if !matches!(
            panel,
            Panel::Status | Panel::Branches | Panel::RemoteBranches | Panel::Details
        ) {
            self.request_preview(panel == Panel::Diff);
        }
        self.dirty = true;
    }
    fn move_selection(&mut self, delta: isize) {
        if self.focus == Panel::Commits {
            let old = self.selected_graph_row();
            let next = old
                .saturating_add_signed(delta)
                .min(self.length(Panel::Commits));
            if next != old {
                self.select_graph_row(next);
            }
        } else if self.focus == Panel::Details {
            self.scrolls[9] = self.scrolls[9]
                .saturating_add_signed(delta)
                .min(self.details_max_scroll);
        } else if self.focus == Panel::Diff && self.hunk_mode {
            self.scroll_diff(delta);
        } else if self.focus == Panel::Diff {
            self.diff_line = self
                .diff_line
                .saturating_add_signed(delta)
                .min(self.preview_lines.len().saturating_sub(1));
        } else if self.focus == Panel::Output {
            self.output_follow = false;
            self.output_scroll = self
                .output_scroll
                .saturating_add_signed(delta)
                .min(self.output.len().saturating_sub(1));
        } else {
            let index = panel_index(self.focus);
            self.selections[index] = self.selections[index]
                .saturating_add_signed(delta)
                .min(self.length(self.focus).saturating_sub(1));
            self.diff_line = 0;
            self.diff_scroll = 0;
            if !matches!(
                self.focus,
                Panel::Status | Panel::Branches | Panel::RemoteBranches
            ) {
                self.request_preview(false);
            }
            if self.focus == Panel::Stash {
                self.request_stash_files();
            }
        }
        self.dirty = true;
    }

    pub fn page_step(&self) -> usize {
        self.page_sizes[panel_index(self.focus)]
            .saturating_sub(1)
            .max(1)
    }

    fn start_live_search(&mut self) {
        self.search_edit = Some(SearchEdit {
            panel: self.focus,
            previous: self.search(self.focus).to_owned(),
            selection: self.selected(self.focus),
            scroll: self.scrolls[panel_index(self.focus)],
            working_tree: self.graph_working_tree,
            branch_row: matches!(self.focus, Panel::Branches | Panel::RemoteBranches)
                .then(|| {
                    self.selected_branch_row()
                        .map(|row| (row.path.clone(), row.is_group()))
                })
                .flatten(),
        });
        self.dirty = true;
    }

    fn apply_live_search(&mut self, query: String) {
        let index = panel_index(self.focus);
        if self.searches[index] == query {
            return;
        }
        self.searches[index] = query;
        self.selections[index] = 0;
        self.scrolls[index] = 0;
        match self.focus {
            Panel::Commits => {
                self.graph_working_tree = self.filtered_commits().is_empty();
                self.sync_inspection();
            }
            Panel::Files => self.rebuild(),
            Panel::CommitFiles => {
                if self.stash_context {
                    self.rebuild_stash_rows();
                } else {
                    self.rebuild_commit_rows();
                }
            }
            Panel::HeadFiles => self.rebuild_head_rows(),
            Panel::Branches | Panel::RemoteBranches => self.rebuild_branch_rows(),
            Panel::Stash => self.request_stash_files(),
            _ => {}
        }
        self.diff_line = 0;
        self.diff_scroll = 0;
        self.preview_key = None;
        self.request_preview(false);
        self.dirty = true;
    }

    fn search_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => {
                self.search_edit = None;
            }
            KeyCode::Esc | KeyCode::Char('c')
                if key.code == KeyCode::Esc || key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                let edit = self.search_edit.take().unwrap();
                self.apply_live_search(edit.previous);
                self.selections[panel_index(edit.panel)] = edit.selection;
                self.scrolls[panel_index(edit.panel)] = edit.scroll;
                self.graph_working_tree = edit.working_tree;
                if let Some((path, group)) = edit.branch_row {
                    let remote = edit.panel == Panel::RemoteBranches;
                    if let Some(index) = self
                        .branch_rows(remote)
                        .iter()
                        .position(|row| row.path == path && row.is_group() == group)
                    {
                        self.selections[panel_index(edit.panel)] = index;
                    }
                }
                self.clamp_selections();
                if edit.panel == Panel::Commits {
                    self.sync_inspection();
                }
                if edit.panel == Panel::Stash {
                    self.request_stash_files();
                }
                self.preview_key = None;
                self.request_preview(false);
            }
            KeyCode::Down | KeyCode::Up | KeyCode::PageDown | KeyCode::PageUp => {
                let delta = match key.code {
                    KeyCode::Down => 1,
                    KeyCode::Up => -1,
                    KeyCode::PageDown => self.page_step() as isize,
                    _ => -(self.page_step() as isize),
                };
                self.move_selection(delta);
            }
            KeyCode::Char('n' | 'p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.move_selection(if key.code == KeyCode::Char('n') {
                    1
                } else {
                    -1
                });
            }
            _ => {
                let mut query = self.search(self.focus).to_owned();
                match key.code {
                    KeyCode::Backspace => {
                        if key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                        {
                            while query.ends_with(char::is_whitespace) {
                                query.pop();
                            }
                            while !query.is_empty() && !query.ends_with(char::is_whitespace) {
                                query.pop();
                            }
                        } else {
                            query.pop();
                        }
                    }
                    KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        query.clear()
                    }
                    KeyCode::Char(character)
                        if !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER) =>
                    {
                        query.push(character)
                    }
                    _ => return,
                }
                self.apply_live_search(query);
            }
        }
        self.dirty = true;
    }

    pub fn search_hint(&self, panel: Panel) -> Option<String> {
        let editing = self
            .search_edit
            .as_ref()
            .is_some_and(|edit| edit.panel == panel);
        let query = self.search(panel);
        if !editing && query.is_empty() {
            return None;
        }
        let count = match panel {
            Panel::Commits => self.filtered_commits().len(),
            Panel::Files => self.rows.iter().filter(|row| !row.directory).count(),
            Panel::CommitFiles => self
                .file_rows(panel)
                .iter()
                .filter(|row| !row.directory)
                .count(),
            Panel::HeadFiles => self.head_rows.iter().filter(|row| !row.directory).count(),
            Panel::Branches if self.branch_tab != 3 => self.refs_for(false).len(),
            Panel::RemoteBranches => self.refs_for(true).len(),
            _ => return None,
        };
        Some(format!(
            " /{query}{} · {count} matches{} ",
            if editing { "▏" } else { "" },
            if panel == Panel::Commits {
                " (loaded)"
            } else {
                ""
            }
        ))
    }

    fn scroll_diff(&mut self, delta: isize) {
        let last_line = self.preview_lines.len().saturating_sub(1);
        self.diff_line = self.diff_line.saturating_add_signed(delta).min(last_line);
        let max_scroll = self
            .preview_lines
            .len()
            .saturating_sub(self.diff_view_height.max(1));
        self.diff_scroll = self
            .diff_scroll
            .saturating_add_signed(delta)
            .min(max_scroll);
    }
    fn prompt(&mut self, title: impl Into<String>, purpose: Prompt, text: String) {
        self.modal = Some(Modal::Input {
            title: title.into(),
            purpose,
            text,
        });
    }
    fn confirm(&mut self, operation: Operation) {
        if self.warn_if_busy(&operation.label) {
            return;
        }
        self.modal = Some(Modal::Confirm {
            title: operation.label.clone(),
            operation,
        });
    }
    fn selected_files(&self) -> Vec<&FileEntry> {
        let files = if self.stash_context && self.side == Panel::CommitFiles {
            &self.stash_files
        } else if self.side == Panel::CommitFiles {
            &self.commit_files
        } else {
            &self.snapshot.files
        };
        let marks = self.file_marks();
        if !marks.is_empty() {
            return files
                .iter()
                .filter(|file| marks.iter().any(|path| file.path.starts_with(path)))
                .collect();
        }
        self.row()
            .map(|row| tree::matching_files(files, &row.path))
            .unwrap_or_default()
    }
    fn discard_selected_files(&mut self) -> Result<()> {
        if self.stash_context {
            bail!("Stashed files are read only; apply the stash before discarding a file");
        }
        if self.comparison.is_some() {
            bail!(
                "Comparison files are read only. Close the comparison before discarding changes."
            );
        }
        let selected = self.selected_files();
        let files = if self.side == Panel::CommitFiles {
            self.snapshot
                .files
                .iter()
                .filter(|file| {
                    selected.iter().any(|selected| {
                        std::iter::once(&selected.path)
                            .chain(selected.old_path.iter())
                            .any(|path| file.path == *path || file.old_path.as_ref() == Some(path))
                    })
                })
                .collect()
        } else {
            selected
        };
        self.confirm(discard_operation(&files, !self.snapshot.head.is_empty())?);
        Ok(())
    }
    fn selected_file_paths(&self) -> Vec<PathBuf> {
        let mut paths: Vec<_> = self
            .selected_files()
            .into_iter()
            .flat_map(|file| std::iter::once(file.path.clone()).chain(file.old_path.clone()))
            .collect();
        paths.sort_unstable();
        paths.dedup();
        paths
    }
    fn stage(&mut self, all: bool) -> Result<()> {
        let files = if all {
            self.snapshot
                .files
                .iter()
                .filter(|file| file.changed())
                .collect()
        } else {
            self.selected_files()
        };
        let unstage =
            files.iter().any(|file| file.staged()) && !files.iter().any(|file| file.unstaged());
        let operation = stage_operation(&files, unstage, !self.snapshot.head.is_empty())?;
        self.run_operation(operation);
        Ok(())
    }

    fn execute_menu(&mut self, action: MenuAction) {
        match action {
            MenuAction::PullMenu => self.pull_menu(),
            MenuAction::PushMenu(reference) => self.push_menu(reference),
            MenuAction::Run(operation, confirm) => {
                if confirm {
                    self.confirm(operation)
                } else {
                    self.run_operation(operation)
                }
            }
            MenuAction::External(command) => self.pending_external = Some(command),
            MenuAction::Input(prompt) => self.prompt_for(prompt),
            MenuAction::SwitchRepo(path) => self.pending_repo = Some(path),
            MenuAction::HistoryClear => {
                self.options.history_path = None;
                self.loading = false;
                self.refresh();
            }
            MenuAction::HistoryLimit(limit) => {
                if let Err(error) = self.set_history_limit(limit) {
                    self.error(format!("{error:#}"));
                }
            }
            MenuAction::HistoryMore => self.load_more_history(),
            MenuAction::HistoryRetry => {
                self.history_key = None;
                self.request_history();
            }
            MenuAction::ToggleLog => {
                self.toggle_details_tab();
            }
            MenuAction::ToggleUnchanged => {
                self.config.gui.show_unchanged_files = !self.config.gui.show_unchanged_files;
                self.rebuild();
            }
            MenuAction::ToggleFilter => {
                self.changed_only = !self.changed_only;
                self.rebuild();
            }
            MenuAction::Copy(text) => self.copy(text),
            MenuAction::CompareMark(revision) => {
                self.notice = format!(
                    "Comparison start: {} · select another revision, then B → Compare marked",
                    revision.name
                );
                self.comparison_mark = Some(revision);
            }
            MenuAction::Compare(request) => self.start_comparison(request),
            MenuAction::CompareClose => self.close_comparison(),
            MenuAction::CompareUnmark => {
                self.comparison_mark = None;
                self.notice = "Comparison start cleared".into();
            }
        }
    }

    fn prompt_for(&mut self, purpose: Prompt) {
        let title = match &purpose {
            Prompt::Search => "Filter (subsequence search)",
            Prompt::Commit(false) => "Commit message",
            Prompt::Commit(true) => "Amend commit message",
            Prompt::Branch(_) => "New branch name",
            Prompt::RenameBranch(_) => "Rename branch",
            Prompt::CheckoutBranch => "Checkout branch by name",
            Prompt::Tag(_) => "New tag name",
            Prompt::Stash => "Stash message (includes untracked files)",
            Prompt::Shell(false) => "Run command (live output in pane 8)",
            Prompt::Shell(true) => "Run interactive command",
            Prompt::HistoryPath => "Filter commit history by repository-relative path",
            Prompt::HistoryLimit => {
                "Commit initial/page size (1–1,000,000; automatic paging stays on)"
            }
            Prompt::CompareFrom => "Compare from (A): commit, branch or tag",
            Prompt::CompareTo(_) => "Compare to (B): commit, branch or tag",
            Prompt::CompareBase(_) => "Base branch: compare common ancestor → selected revision",
            Prompt::Worktree(_) => "New worktree path",
            Prompt::Custom { command, index, .. } => command
                .prompts
                .get(*index)
                .and_then(|prompt| prompt.get("title"))
                .and_then(|title| title.as_str())
                .unwrap_or("Command argument"),
        }
        .to_owned();
        let text = match &purpose {
            Prompt::Search => self.search(self.focus).to_owned(),
            Prompt::Commit(true) => self
                .snapshot
                .commits
                .first()
                .map(|commit| commit.subject.clone())
                .unwrap_or_default(),
            Prompt::RenameBranch(name) => name.clone(),
            Prompt::HistoryLimit => self.history_page_size().to_string(),
            Prompt::CompareFrom => "HEAD".into(),
            Prompt::CompareTo(_) => self
                .comparison_revision()
                .map(|revision| revision.name)
                .unwrap_or_else(|| "HEAD".into()),
            Prompt::CompareBase(_) => ["main", "master", "origin/main", "origin/master"]
                .into_iter()
                .find(|name| {
                    self.snapshot
                        .references
                        .iter()
                        .any(|reference| reference.name == *name)
                })
                .unwrap_or("HEAD")
                .into(),
            _ => String::new(),
        };
        self.prompt(title, purpose, text);
    }

    fn submit_prompt(&mut self, purpose: Prompt, text: String) -> Result<()> {
        match purpose {
            Prompt::Search => {
                self.searches[panel_index(self.focus)] = text;
                self.selections[panel_index(self.focus)] = 0;
                if matches!(self.focus, Panel::Diff | Panel::Output) {
                    self.diff_line = 0;
                    self.output_scroll = 0;
                    self.find_match(true, true);
                } else {
                    self.rebuild();
                    if self.focus == Panel::Commits {
                        self.graph_working_tree = false;
                        self.sync_inspection();
                    }
                    self.request_preview(false);
                }
            }
            Prompt::CompareFrom => {
                if text.trim().is_empty() {
                    bail!("Enter the starting revision");
                }
                self.prompt_for(Prompt::CompareTo(Revision::named(text.trim().into())));
            }
            Prompt::CompareTo(from) => {
                if text.trim().is_empty() {
                    bail!("Enter the target revision");
                }
                self.start_comparison(ComparisonRequest {
                    from,
                    to: Revision::named(text.trim().into()),
                    merge_base: false,
                });
            }
            Prompt::CompareBase(to) => {
                if text.trim().is_empty() {
                    bail!("Enter a base branch or commit");
                }
                self.start_comparison(ComparisonRequest {
                    from: Revision::named(text.trim().into()),
                    to,
                    merge_base: true,
                });
            }
            Prompt::Commit(amend) => {
                if text.trim().is_empty() {
                    bail!("Commit message cannot be empty");
                }
                let mut args = vec!["commit".into(), "-m".into(), OsString::from(text)];
                if amend {
                    args.push("--amend".into());
                }
                let operation = Operation::git(args);
                if amend {
                    self.confirm(operation);
                } else {
                    self.run_operation(operation);
                }
            }
            Prompt::Branch(start) => {
                validate_name(&text)?;
                let mut args = vec![OsString::from("switch"), "-c".into(), text.into()];
                if !start.is_empty() {
                    args.push(start.into());
                }
                self.run_operation(Operation::git(args));
            }
            Prompt::RenameBranch(old) => {
                validate_name(&text)?;
                self.run_operation(Operation::git(["branch", "-m", &old, &text]));
            }
            Prompt::CheckoutBranch => {
                validate_name(&text)?;
                self.run_operation(Operation::git(["switch", "--", &text]));
            }
            Prompt::Tag(oid) => {
                validate_name(&text)?;
                self.run_operation(Operation::git(["tag", &text, &oid]));
            }
            Prompt::Stash => self.run_operation(Operation::git([
                "stash",
                "push",
                "--include-untracked",
                "-m",
                &text,
            ])),
            Prompt::Shell(interactive) => {
                if interactive {
                    self.pending_external = Some(CommandSpec::shell(text));
                } else {
                    self.run_operation(Operation::shell(text));
                }
            }
            Prompt::HistoryPath => {
                let path = PathBuf::from(text);
                crate::git::validate_path(&path)?;
                self.options.history_directory = self.repo.root.join(&path).is_dir();
                self.options.history_path = Some(path);
                self.graph_working_tree = false;
                self.set_focus(Panel::Commits);
                self.loading = false;
                self.refresh();
            }
            Prompt::Worktree(branch) => {
                let mut args = vec!["worktree".into(), "add".into(), OsString::from(text)];
                if !branch.is_empty() {
                    args.push(branch.into());
                }
                self.run_operation(Operation::git(args));
            }
            Prompt::HistoryLimit => {
                let limit = text
                    .trim()
                    .parse::<usize>()
                    .context("Enter a positive whole number of commits")?;
                self.set_history_limit(limit)?;
            }
            Prompt::Custom {
                command,
                mut values,
                index,
            } => {
                values.push(text);
                if index + 1 < command.prompts.len() {
                    self.prompt_for(Prompt::Custom {
                        command,
                        values,
                        index: index + 1,
                    });
                } else {
                    self.run_custom(command, &values)?;
                }
            }
        }
        Ok(())
    }

    fn handle_modal(&mut self, key: KeyEvent) -> Result<()> {
        let Some(modal) = self.modal.take() else {
            return Ok(());
        };
        match modal {
            Modal::Busy { running, requested } => {
                if !matches!(key.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q')) {
                    self.modal = Some(Modal::Busy { running, requested });
                }
            }
            Modal::Input {
                title,
                mut text,
                purpose,
            } => {
                match key.code {
                    KeyCode::Esc => return Ok(()),
                    KeyCode::Enter => return self.submit_prompt(purpose, text),
                    KeyCode::Backspace => {
                        if key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                        {
                            while text.ends_with(char::is_whitespace) {
                                text.pop();
                            }
                            while !text.is_empty() && !text.ends_with(char::is_whitespace) {
                                text.pop();
                            }
                        } else {
                            text.pop();
                        }
                    }
                    KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        text.clear()
                    }
                    KeyCode::Char(character)
                        if !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER) =>
                    {
                        text.push(character)
                    }
                    _ => {}
                }
                self.modal = Some(Modal::Input {
                    title,
                    text,
                    purpose,
                });
            }
            Modal::Confirm { title, operation } => match key.code {
                KeyCode::Char('y') | KeyCode::Enter => self.run_operation(operation),
                KeyCode::Esc | KeyCode::Char('n') => {}
                _ => self.modal = Some(Modal::Confirm { title, operation }),
            },
            Modal::Menu {
                title,
                items,
                mut selected,
            } => {
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') => return Ok(()),
                    KeyCode::Enter => {
                        if let Some(item) = items.get(selected) {
                            self.execute_menu(item.action.clone());
                        }
                        return Ok(());
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        selected = (selected + 1).min(items.len().saturating_sub(1))
                    }
                    KeyCode::Up | KeyCode::Char('k') => selected = selected.saturating_sub(1),
                    KeyCode::Char(number) if ('1'..='9').contains(&number) => {
                        if let Some(item) = items.get(number as usize - '1' as usize) {
                            self.execute_menu(item.action.clone());
                        }
                        return Ok(());
                    }
                    _ => {}
                }
                self.modal = Some(Modal::Menu {
                    title,
                    items,
                    selected,
                });
            }
            Modal::Help { mut scroll } => {
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') => return Ok(()),
                    KeyCode::Down | KeyCode::Char('j') => scroll += 1,
                    KeyCode::Up | KeyCode::Char('k') => scroll = scroll.saturating_sub(1),
                    KeyCode::PageDown => scroll += 15,
                    KeyCode::PageUp => scroll = scroll.saturating_sub(15),
                    _ => {}
                }
                self.modal = Some(Modal::Help { scroll });
            }
            Modal::Actions {
                title,
                items,
                mut selected,
            } => {
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') => return Ok(()),
                    KeyCode::Enter => {
                        if let Some(item) = items.get(selected) {
                            return self.key(item.event);
                        }
                        return Ok(());
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        selected = (selected + 1).min(items.len().saturating_sub(1))
                    }
                    KeyCode::Up | KeyCode::Char('k') => selected = selected.saturating_sub(1),
                    KeyCode::PageDown => {
                        selected = (selected + 10).min(items.len().saturating_sub(1))
                    }
                    KeyCode::PageUp => selected = selected.saturating_sub(10),
                    KeyCode::Home => selected = 0,
                    KeyCode::End => selected = items.len().saturating_sub(1),
                    _ => {
                        let modifiers = if matches!(key.code, KeyCode::Char(character) if character.is_uppercase())
                        {
                            key.modifiers - KeyModifiers::SHIFT
                        } else {
                            key.modifiers
                        };
                        if let Some(item) = items.iter().find(|item| {
                            item.event.code == key.code && item.event.modifiers == modifiers
                        }) {
                            return self.key(item.event);
                        }
                    }
                }
                self.modal = Some(Modal::Actions {
                    title,
                    items,
                    selected,
                });
            }
            Modal::Message(message) => {
                match key.code {
                    KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => {
                        self.message_scroll = 0;
                        return Ok(());
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        self.message_scroll = (self.message_scroll + 1).min(self.message_max_scroll)
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        self.message_scroll = self.message_scroll.saturating_sub(1)
                    }
                    KeyCode::PageDown => {
                        self.message_scroll = self
                            .message_scroll
                            .saturating_add(self.message_page_size.saturating_sub(1).max(1))
                            .min(self.message_max_scroll)
                    }
                    KeyCode::PageUp => {
                        self.message_scroll = self
                            .message_scroll
                            .saturating_sub(self.message_page_size.saturating_sub(1).max(1))
                    }
                    KeyCode::Home => self.message_scroll = 0,
                    KeyCode::End => self.message_scroll = self.message_max_scroll,
                    _ => {}
                }
                self.modal = Some(Modal::Message(message));
            }
        }
        Ok(())
    }

    pub fn paste(&mut self, text: String) {
        if self.search_edit.is_some() {
            let mut query = self.search(self.focus).to_owned();
            query.push_str(&text.replace(['\r', '\n'], " "));
            self.apply_live_search(query);
        } else if let Some(Modal::Input { text: input, .. }) = &mut self.modal {
            input.push_str(&text.replace(['\r', '\n'], " "));
            self.dirty = true;
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        self.dirty = true;
        if let Err(error) = self.key(key) {
            self.error(format!("{error:#}"));
        }
    }

    fn key(&mut self, key: KeyEvent) -> Result<()> {
        if key_matches("<ctrl+c>", key) && self.busy.is_some() {
            self.busy.as_ref().unwrap().0.cancel();
            self.notice = "Cancelling command…".into();
            self.modal = None;
            return Ok(());
        }
        if self.modal.is_some() {
            return self.handle_modal(key);
        }
        if self.search_edit.is_some() {
            self.search_key(key);
            return Ok(());
        }
        if key_matches("<ctrl+c>", key) {
            if let Some((cancel, _, _)) = &self.busy {
                cancel.cancel();
                self.notice = "Cancelling command…".into();
            } else {
                self.quit = true;
            }
            return Ok(());
        }
        if self.is("universal", "quit", &["q"], key) {
            if self.busy.is_some() {
                self.notice = "Ctrl+c cancels the active command before quitting.".into();
            } else if self.config.confirm_on_quit {
                self.modal = Some(Modal::Message(
                    "Press Ctrl+c to quit, or Esc to continue.".into(),
                ));
            } else {
                self.quit = true;
            }
            return Ok(());
        }
        if self.is("universal", "return", &["<esc>"], key) {
            if self.focus == Panel::Diff {
                self.clear_file_marks();
                self.fullscreen = false;
                self.set_focus(self.side);
            } else if matches!(self.focus, Panel::Files | Panel::CommitFiles)
                && self.clear_file_marks()
            {
            } else if self.comparison.is_some() && self.search(self.focus).is_empty() {
                self.close_comparison();
            } else if matches!(self.focus, Panel::CommitFiles | Panel::HeadFiles)
                && self.search(self.focus).is_empty()
            {
                self.set_focus(Panel::Commits);
            } else if !self.search(self.focus).is_empty() {
                let selected_oid = self.selected_commit().map(|commit| commit.oid.clone());
                self.searches[panel_index(self.focus)].clear();
                self.rebuild();
                if self.focus == Panel::Commits {
                    if let Some(oid) = selected_oid {
                        self.selections[3] = self
                            .filtered_commits()
                            .iter()
                            .position(|index| self.snapshot.commits[*index].oid == oid)
                            .unwrap_or(0);
                    }
                    self.sync_inspection();
                }
                self.request_preview(false);
            } else {
                self.set_focus(Panel::Commits);
            }
            return Ok(());
        }
        if let Some(command) = self
            .config
            .custom_commands
            .iter()
            .find(|command| {
                key_matches(&command.key, key)
                    && command.context.split(',').any(|context| {
                        context.trim() == "global" || context.trim() == self.focus.context()
                    })
            })
            .cloned()
        {
            if command.prompts.is_empty() {
                self.run_custom(command, &[])?;
            } else {
                self.prompt_for(Prompt::Custom {
                    command,
                    values: Vec::new(),
                    index: 0,
                });
            }
            return Ok(());
        }
        if self.is("universal", "contextActions", &["O"], key)
            || (key_matches("o", key) && !actions::file_opener_context(self))
        {
            self.modal = Some(Modal::Actions {
                title: format!(
                    "Actions · {}",
                    if self.focus == Panel::Commits && self.graph_working_tree {
                        "Working tree / Index"
                    } else {
                        self.focus.title()
                    }
                ),
                items: actions::for_app(self),
                selected: 0,
            });
            return Ok(());
        }
        if self.is("universal", "compareRevisions", &["B"], key) {
            self.comparison_menu();
            return Ok(());
        }
        for (number, panel) in [
            ('1', Panel::Status),
            ('2', Panel::Branches),
            ('3', Panel::RemoteBranches),
            ('4', Panel::Stash),
            ('5', Panel::Commits),
            ('6', self.details_panel()),
            ('7', self.file_panel()),
            ('8', Panel::Output),
        ] {
            if key.code == KeyCode::Char(number) && key.modifiers.is_empty() {
                self.set_focus(panel);
                return Ok(());
            }
        }
        if self.is("universal", "focusMainView", &["0"], key) {
            self.set_focus(Panel::Diff);
            return Ok(());
        }
        if self.is("universal", "optionMenu", &["?"], key) {
            self.modal = Some(Modal::Help { scroll: 0 });
            return Ok(());
        }
        let live_search_panel = matches!(
            self.focus,
            Panel::Commits
                | Panel::Files
                | Panel::CommitFiles
                | Panel::HeadFiles
                | Panel::RemoteBranches
        ) || (self.focus == Panel::Branches && self.branch_tab != 3);
        if live_search_panel
            && (self.is("universal", "startSearch", &["/"], key)
                || self.is("universal", "fuzzySearch", &["<ctrl+f>"], key))
        {
            self.start_live_search();
            return Ok(());
        }
        if self.is("universal", "startSearch", &["/"], key) {
            self.prompt_for(Prompt::Search);
            return Ok(());
        }
        if matches!(self.focus, Panel::Diff | Panel::Output) && !self.search(self.focus).is_empty()
        {
            if self.is("universal", "nextMatch", &["n"], key) {
                self.find_match(true, false);
                return Ok(());
            }
            if self.is("universal", "prevMatch", &["N"], key) {
                self.find_match(false, false);
                return Ok(());
            }
        }
        if self.is("universal", "executeShellCommand", &[":"], key) {
            self.prompt_for(Prompt::Shell(false));
            return Ok(());
        }
        if key_matches("!", key) {
            self.prompt_for(Prompt::Shell(true));
            return Ok(());
        }
        if self.is("universal", "openRecentRepos", &["<ctrl+r>"], key) {
            self.recent_menu();
            return Ok(());
        }
        if self.is("universal", "filteringMenu", &["<ctrl+s>"], key) {
            self.filter_menu();
            return Ok(());
        }
        if self.is("universal", "extrasMenu", &["@"], key) {
            self.menu(
                "Command output",
                vec![MenuItem {
                    label: "Switch Details / Output tab in pane 6".into(),
                    action: MenuAction::ToggleLog,
                }],
            );
            return Ok(());
        }
        if self.is("universal", "toggleDiffPreview", &["~"], key) {
            self.state.diff_preview_enabled = !self.state.diff_preview_enabled;
            self.preview_key = None;
            self.save_state();
            self.request_preview(self.focus == Panel::Diff);
            return Ok(());
        }
        if self.is("universal", "nextScreenMode", &["+"], key) {
            self.fullscreen = !self.fullscreen;
            return Ok(());
        }
        if matches!(
            self.focus,
            Panel::Files | Panel::CommitFiles | Panel::HeadFiles
        ) {
            if self.is("files", "toggleAllDirectories", &["*"], key) {
                self.set_all_directories(None);
                return Ok(());
            }
            if self.is("files", "collapseAll", &["-"], key) {
                self.set_all_directories(Some(true));
                return Ok(());
            }
            if self.is("files", "expandAll", &["="], key) {
                self.set_all_directories(Some(false));
                return Ok(());
            }
        }
        if matches!(self.focus, Panel::Files | Panel::CommitFiles) && key_matches("v", key) {
            self.mark_current_file(true);
            self.preview_key = None;
            self.request_preview(false);
            return Ok(());
        }
        if self.is("universal", "prevItem", &["<up>"], key)
            || self.is("universal", "prevItem-alt", &["k"], key)
        {
            self.move_selection(-1);
            return Ok(());
        }
        if self.is("universal", "nextItem", &["<down>"], key)
            || self.is("universal", "nextItem-alt", &["j"], key)
        {
            self.move_selection(1);
            return Ok(());
        }
        if key_matches("<shift+down>", key) || key_matches("<shift+up>", key) {
            if matches!(self.focus, Panel::Files | Panel::CommitFiles) {
                self.mark_current_file(false);
                self.move_selection(if key.code == KeyCode::Down { 1 } else { -1 });
                self.mark_current_file(false);
                self.preview_key = None;
                self.request_preview(false);
            } else if self.focus == Panel::Commits {
                self.move_selection(if key.code == KeyCode::Down {
                    self.page_step() as isize
                } else {
                    -(self.page_step() as isize)
                });
            }
            return Ok(());
        }
        if self.is("universal", "scrollUpMain", &["<pgup>"], key) || key_matches("<ctrl+u>", key) {
            if self.focus == Panel::Diff {
                self.scroll_diff(-(self.diff_view_height.saturating_sub(1).max(1) as isize));
            } else {
                self.move_selection(-(self.page_step() as isize));
            }
            return Ok(());
        }
        if self.is("universal", "scrollDownMain", &["<pgdown>"], key)
            || key_matches("<ctrl+d>", key)
        {
            if self.focus == Panel::Diff {
                self.scroll_diff(self.diff_view_height.saturating_sub(1).max(1) as isize);
            } else {
                self.move_selection(self.page_step() as isize);
            }
            return Ok(());
        }
        if key_matches("<home>", key) || key_matches("<", key) {
            self.move_selection(-(isize::MAX));
            return Ok(());
        }
        if key_matches("<end>", key) || key_matches(">", key) {
            self.move_selection(isize::MAX);
            if self.focus == Panel::Output {
                self.output_follow = true;
            }
            return Ok(());
        }
        if self.is("universal", "nextBlock", &["<right>"], key)
            || self.is("universal", "nextBlock-alt", &["l"], key)
            || key_matches("<tab>", key)
        {
            if self.focus == Panel::Diff && key.code != KeyCode::Tab {
                self.next_hunk(1);
            } else if key.code != KeyCode::Tab && self.navigate_branch_group(true) {
            } else if matches!(
                self.focus,
                Panel::Files | Panel::CommitFiles | Panel::HeadFiles
            ) && key.code != KeyCode::Tab
                && self.row().is_some_and(|row| row.directory)
            {
                let path = self.row().unwrap().path.clone();
                self.collapsed.remove(&path);
                self.rebuild();
            } else {
                self.cycle_panel(1);
            }
            return Ok(());
        }
        if self.is("universal", "prevBlock", &["<left>"], key)
            || self.is("universal", "prevBlock-alt", &["h"], key)
            || key_matches("<backtab>", key)
        {
            if self.focus == Panel::Diff && key.code != KeyCode::BackTab {
                self.next_hunk(-1);
            } else if key.code != KeyCode::BackTab && self.navigate_branch_group(false) {
            } else if matches!(
                self.focus,
                Panel::Files | Panel::CommitFiles | Panel::HeadFiles
            ) && key.code != KeyCode::BackTab
                && self.row().is_some_and(|row| row.directory)
            {
                let path = self.row().unwrap().path.clone();
                self.collapsed.insert(path);
                self.rebuild();
            } else {
                self.cycle_panel(-1);
            }
            return Ok(());
        }
        if self.is("universal", "nextTab", &["]"], key)
            || self.is("universal", "prevTab", &["["], key)
        {
            let previous = key.code == KeyCode::Char('[');
            match self.focus {
                Panel::Branches => {
                    self.branch_tab = match (self.branch_tab, previous) {
                        (0, false) | (3, true) => 2,
                        (2, false) | (0, true) => 3,
                        _ => 0,
                    };
                    self.selections[2] = 0;
                    self.scrolls[2] = 0;
                    self.branch_rows_cache[0].clear();
                    self.rebuild_branch_rows();
                }
                Panel::Commits => {
                    self.commit_tab = 1 - self.commit_tab;
                    self.loading = false;
                    self.refresh();
                }
                Panel::Diff => {
                    if self.side != Panel::HeadFiles && self.comparison.is_none() {
                        self.show_staged = !self.show_staged;
                        self.preview_key = None;
                        self.request_preview(true);
                    }
                }
                Panel::Files | Panel::CommitFiles | Panel::HeadFiles => self.toggle_file_tab(),
                Panel::Details | Panel::Output => self.toggle_details_tab(),
                _ => {}
            }
            self.request_preview(self.focus == Panel::Diff);
            return Ok(());
        }
        if self.is("universal", "viewFileHistory", &["F"], key)
            && matches!(
                self.focus,
                Panel::Files | Panel::CommitFiles | Panel::HeadFiles | Panel::Diff
            )
        {
            if let Some(row) = self.row().cloned() {
                self.options.history_path = Some(row.path);
                self.options.history_directory = row.directory;
                self.commit_tab = 0;
                self.graph_working_tree = false;
                self.set_focus(Panel::Commits);
                self.loading = false;
                self.refresh();
            }
            return Ok(());
        }
        if self.focus == Panel::Commits && self.is("commits", "setHistoryLimit", &["L"], key) {
            self.history_limit_menu();
            return Ok(());
        }
        if self.focus == Panel::Commits && key_matches(".", key) {
            self.load_more_history();
            return Ok(());
        }
        if self.focus == Panel::Commits && key_matches("G", key) {
            self.run_operation(Operation::git([
                "commit-graph",
                "write",
                "--reachable",
                "--progress",
            ]));
            return Ok(());
        }
        if self.is("universal", "openDiffTool", &["<ctrl+t>"], key) {
            self.external_tool(false)?;
            return Ok(());
        }
        if self.is("universal", "refresh", &["R"], key)
            && !matches!(
                self.focus,
                Panel::Branches | Panel::RemoteBranches | Panel::Commits
            )
        {
            self.loading = false;
            self.refresh();
            return Ok(());
        }
        if self.is("universal", "pull", &["p"], key) {
            self.pull_menu();
            return Ok(());
        }
        if self.is("universal", "push", &["P"], key) {
            let reference = matches!(self.focus, Panel::Branches | Panel::RemoteBranches)
                .then(|| self.selected_ref().cloned())
                .flatten()
                .filter(|reference| matches!(reference.kind, RefKind::Branch | RefKind::Tag));
            self.push_menu(reference);
            return Ok(());
        }
        if self.is("universal", "createRebaseOptionsMenu", &["m"], key) {
            self.operation_menu();
            return Ok(());
        }
        match self.focus {
            Panel::Files => self.files_key(key)?,
            Panel::CommitFiles => self.commit_files_key(key)?,
            Panel::HeadFiles => self.head_files_key(key)?,
            Panel::Diff => self.diff_key(key)?,
            Panel::Branches | Panel::RemoteBranches => self.branches_key(key)?,
            Panel::Commits if self.graph_working_tree => {
                if key.code == KeyCode::Enter {
                    self.set_focus(self.file_panel());
                } else {
                    let side = self.side;
                    self.side = Panel::Files;
                    let result = self.files_key(key);
                    self.side = side;
                    result?;
                }
            }
            Panel::Commits => self.commits_key(key)?,
            Panel::Stash => self.stash_key(key)?,
            Panel::Status if key.code == KeyCode::Enter => {
                if let Some(path) = self.state.recent_repos.get(self.selected(Panel::Status)) {
                    self.pending_repo = Some(path.clone());
                }
            }
            Panel::Status if key.code == KeyCode::Char('r') => self.repository_menu(),
            Panel::Details if key.code == KeyCode::Enter => self.set_focus(self.file_panel()),
            _ => {}
        }
        Ok(())
    }

    fn cycle_panel(&mut self, delta: isize) {
        let panels = [
            Panel::Status,
            Panel::Branches,
            Panel::RemoteBranches,
            Panel::Stash,
            Panel::Commits,
            self.details_panel(),
            self.file_panel(),
            Panel::Diff,
        ];
        let index = panels
            .iter()
            .position(|panel| *panel == self.focus)
            .unwrap_or(1);
        self.set_focus(panels[(index as isize + delta).rem_euclid(panels.len() as isize) as usize]);
    }
    fn next_hunk(&mut self, direction: isize) {
        let current = self.patch.hunk_at(self.diff_line);
        let next = match (current, direction) {
            (Some(index), 1) => index
                .saturating_add(1)
                .min(self.patch.hunks.len().saturating_sub(1)),
            (Some(index), _) => index.saturating_sub(1),
            _ => 0,
        };
        if let Some(hunk) = self.patch.hunks.get(next) {
            self.diff_line = hunk.lines.start;
        }
    }
    fn find_match(&mut self, forward: bool, include_current: bool) {
        let pattern = self.search(self.focus);
        if pattern.is_empty() {
            return;
        }
        let (length, current) = if self.focus == Panel::Diff {
            (self.preview_lines.len(), self.diff_line)
        } else {
            (self.output.len(), self.output_scroll)
        };
        if length == 0 {
            return;
        }
        let begin = usize::from(!include_current);
        let found = (begin..length + begin)
            .map(|offset| {
                if forward {
                    (current + offset) % length
                } else {
                    (current + length - offset % length) % length
                }
            })
            .find(|index| {
                let text = if self.focus == Panel::Diff {
                    &self.preview_lines[*index]
                } else {
                    &self.output[*index]
                };
                fuzzy_match(text, pattern)
            });
        if let Some(index) = found {
            if self.focus == Panel::Diff {
                self.diff_line = index;
            } else {
                self.output_scroll = index;
                self.output_follow = false;
            }
        } else {
            self.notice = "No search matches".into();
        }
        self.dirty = true;
    }
    fn menu(&mut self, title: impl Into<String>, items: Vec<MenuItem>) {
        self.modal = Some(Modal::Menu {
            title: title.into(),
            items,
            selected: 0,
        });
    }
}

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() || name.starts_with('-') || name.contains(['\0', '\n', '\r']) {
        bail!("Enter a valid ref name without a leading dash");
    }
    Ok(())
}

impl App {
    fn files_key(&mut self, key: KeyEvent) -> Result<()> {
        if self.is("files", "toggleIgnoredFiles", &["I"], key) {
            self.state.show_ignored = !self.state.show_ignored;
            self.save_state();
            self.loading = false;
            self.refresh();
        } else if self.is("files", "toggleTreeView", &["`"], key) {
            self.config.gui.show_file_tree = !self.config.gui.show_file_tree;
            self.rebuild();
        } else if self.is("files", "collapseAll", &["-"], key) {
            for row in &self.rows {
                if row.directory {
                    self.collapsed.insert(row.path.clone());
                }
            }
            self.rebuild();
        } else if self.is("files", "expandAll", &["="], key) {
            self.collapsed.clear();
            self.rebuild();
        } else if self.is("files", "commitChanges", &["c"], key) {
            self.prompt_for(Prompt::Commit(false));
        } else if self.is("files", "amendLastCommit", &["A"], key) {
            self.prompt_for(Prompt::Commit(true));
        } else if self.is("files", "commitChangesWithEditor", &["C"], key) {
            self.pending_external = Some(CommandSpec::git(["commit"]));
        } else if self.is("files", "commitChangesWithoutHook", &["w"], key) {
            self.pending_external = Some(CommandSpec::git([
                "-c",
                "core.hooksPath=/dev/null",
                "commit",
            ]));
        } else if self.is("files", "stashAllChanges", &["s"], key) {
            self.prompt_for(Prompt::Stash);
        } else if self.is("files", "viewStashOptions", &["S"], key) {
            self.menu(
                "Stash",
                vec![
                    run_item("Stash tracked changes", ["stash", "push"], false),
                    run_item("Stash staged changes", ["stash", "push", "--staged"], false),
                    run_item(
                        "Stash and keep index",
                        ["stash", "push", "--keep-index"],
                        false,
                    ),
                ],
            );
        } else if self.is("files", "toggleStagedAll", &["a"], key) {
            self.stage(true)?;
        } else if self.is("universal", "select", &["<space>"], key) {
            self.stage(false)?;
        } else if self.is("files", "fetch", &["f"], key) {
            self.run_operation(Operation::git(["fetch", "--all", "--prune", "--progress"]));
        } else if self.is("files", "refreshFiles", &["r"], key) {
            self.loading = false;
            self.refresh();
        } else if self.is("files", "openMergeToolForFile", &["<alt+m>"], key) {
            self.external_tool(true)?;
        } else if self.is("universal", "remove", &["d"], key) {
            self.discard_selected_files()?;
        } else if self.is("files", "viewResetOptions", &["D"], key) {
            self.menu(
                "Working tree actions",
                vec![
                    MenuItem {
                        label: "Nuke working tree (stashes submodule changes; keeps ignored files)"
                            .into(),
                        action: MenuAction::Run(crate::git::nuke_working_tree_operation(), true),
                    },
                    run_item(
                        "Discard unstaged tracked changes",
                        ["restore", "--worktree", "--", "."],
                        true,
                    ),
                    run_item("Discard untracked files", ["clean", "-fd"], true),
                    run_item("Unstage all", ["reset", "--mixed", "HEAD"], false),
                    run_item("Soft reset to HEAD", ["reset", "--soft", "HEAD"], false),
                    run_item("Hard reset to HEAD", ["reset", "--hard", "HEAD"], true),
                ],
            );
        } else if self.is("files", "ignoreFile", &["i"], key) {
            if let Some(row) = self.row() {
                let path = row.path.clone();
                self.confirm(Operation {
                    label: format!("Add {} to .gitignore", path.display()),
                    steps: vec![Step::AppendIgnore(path)],
                });
            }
        } else if self.is("files", "openMergeOptions", &["M"], key) {
            self.merge_menu()?;
        } else if self.is("files", "copyFileInfoToClipboard", &["y"], key) {
            if let Some(row) = self.row() {
                self.copy(row.path.to_string_lossy().into_owned());
            }
        } else if key_matches("v", key) {
            if let Some(row) = self.row() {
                let path = row.path.clone();
                if !self.marked.remove(&path) {
                    self.marked.insert(path);
                }
            }
        } else if self.is("universal", "edit", &["e"], key) {
            self.edit_file()?;
        } else if self.is("universal", "openFile", &["o"], key) {
            self.open_file()?;
        } else if key_matches("<enter>", key) {
            self.open_row();
        }
        Ok(())
    }

    fn commit_files_key(&mut self, key: KeyEvent) -> Result<()> {
        if key_matches("<enter>", key) {
            self.open_row();
        } else if key_matches("`", key) {
            self.config.gui.show_file_tree = !self.config.gui.show_file_tree;
            self.rebuild();
        } else if key_matches("e", key) {
            self.edit_file()?;
        } else if key_matches("o", key) {
            self.open_file()?;
        } else if self.is("commitFiles", "cherryPickFiles", &["V"], key) {
            if self.stash_context {
                bail!("Stashed files are read only; apply the stash from pane 4");
            }
            if self.comparison.is_some() {
                bail!(
                    "Comparison files are read only. Close the comparison before cherry-picking files."
                );
            }
            self.confirm(Operation::cherry_pick_files(
                &self.commit_oid,
                &self.selected_files(),
            )?);
        } else if self.is("universal", "remove", &["d"], key) {
            self.discard_selected_files()?;
        } else if self.is("commitFiles", "checkoutCommitFile", &["c"], key) {
            if self.stash_context {
                bail!("Stashed files are read only; apply the stash before restoring a file");
            }
            if self.comparison.is_some() {
                bail!(
                    "Comparison files are read only. Close the comparison before restoring a commit's file."
                );
            }
            let paths = self.selected_file_paths();
            if !paths.is_empty() {
                let mut args = vec![
                    "--literal-pathspecs".into(),
                    "restore".into(),
                    format!("--source={}", self.commit_oid).into(),
                    "--worktree".into(),
                    "--".into(),
                ];
                args.extend(paths.into_iter().map(PathBuf::into_os_string));
                self.confirm(Operation::git(args));
            }
        }
        Ok(())
    }

    fn head_files_key(&mut self, key: KeyEvent) -> Result<()> {
        if key_matches("<enter>", key) {
            self.open_row();
        } else if self.is("files", "toggleTreeView", &["`"], key) {
            self.config.gui.show_file_tree = !self.config.gui.show_file_tree;
            self.rebuild();
        } else if self.is("files", "collapseAll", &["-"], key) {
            for row in &self.head_rows {
                if row.directory {
                    self.collapsed.insert(row.path.clone());
                }
            }
            self.rebuild_head_rows();
        } else if self.is("files", "expandAll", &["="], key) {
            self.collapsed.clear();
            self.rebuild_head_rows();
        } else if self.is("files", "copyFileInfoToClipboard", &["y"], key) {
            if let Some(row) = self.row() {
                self.copy(row.path.to_string_lossy().into_owned());
            }
        }
        Ok(())
    }

    fn open_row(&mut self) {
        if let Some(row) = self.row() {
            if row.directory {
                let path = row.path.clone();
                if !self.collapsed.remove(&path) {
                    self.collapsed.insert(path);
                }
                self.rebuild();
            } else {
                self.fullscreen = true;
                self.set_focus(Panel::Diff);
            }
        }
    }

    fn diff_key(&mut self, key: KeyEvent) -> Result<()> {
        if key_matches("H", key) {
            self.horizontal_scroll = self.horizontal_scroll.saturating_sub(4);
        } else if key_matches("L", key) {
            self.horizontal_scroll = self.horizontal_scroll.saturating_add(4);
        } else if self.is("main", "toggleSelectHunk", &["a"], key) {
            self.hunk_mode = !self.hunk_mode;
        } else if self.is("universal", "increaseContextInDiffView", &["}"], key) {
            self.diff_context = (self.diff_context + 3).min(1000);
            self.preview_key = None;
            self.request_preview(true);
        } else if self.is("universal", "decreaseContextInDiffView", &["{"], key) {
            self.diff_context = self.diff_context.saturating_sub(3);
            self.preview_key = None;
            self.request_preview(true);
        } else if self.is(
            "universal",
            "toggleWhitespaceInDiffView",
            &["<ctrl+w>"],
            key,
        ) {
            self.ignore_whitespace = !self.ignore_whitespace;
            self.preview_key = None;
            self.request_preview(true);
        } else if key_matches("<space>", key) || key_matches("d", key) {
            if self.side != Panel::Files {
                bail!(
                    "Staging applies to working-tree diffs; select Working tree / Index in pane 5, then a file in pane 7"
                );
            }
            if self.ignore_whitespace {
                bail!("Turn off whitespace filtering with Ctrl+w before applying a patch");
            }
            if self.preview.suppressed {
                bail!("Open the complete diff before staging a hunk");
            }
            let discard = key_matches("d", key);
            if discard && self.preview.staged {
                bail!("Unstage this hunk before discarding it");
            }
            let patch = if self.hunk_mode {
                self.patch.hunk_patch(
                    self.patch
                        .hunk_at(self.diff_line)
                        .context("Use left/right to select a text hunk")?,
                )?
            } else {
                self.patch.line_patch(self.diff_line, self.preview.staged)?
            };
            let operation = Operation::patch(patch, !discard, discard || self.preview.staged);
            if discard {
                self.confirm(operation);
            } else {
                self.run_operation(operation);
            }
        } else if key_matches("e", key) {
            self.edit_file()?;
        } else if key_matches("o", key) {
            self.open_file()?;
        } else if key_matches("n", key) || key_matches("N", key) {
            let delta = if key_matches("N", key) { -1 } else { 1 };
            let index = panel_index(self.side);
            self.selections[index] = self.selections[index]
                .saturating_add_signed(delta)
                .min(self.length(self.side).saturating_sub(1));
            if self.side == Panel::Stash {
                self.request_stash_files();
            }
            self.diff_line = 0;
            self.diff_scroll = 0;
            self.request_preview(true);
        }
        Ok(())
    }

    fn branches_key(&mut self, key: KeyEvent) -> Result<()> {
        if self.branch_tab == 3 && self.focus == Panel::Branches {
            if let Some(worktree) = self
                .snapshot
                .worktrees
                .get(self.selected(Panel::Branches))
                .cloned()
            {
                if key_matches("<enter>", key) || key_matches("<space>", key) {
                    self.pending_repo = Some(worktree.path);
                } else if key_matches("d", key) {
                    let args = vec![
                        "worktree".into(),
                        "remove".into(),
                        worktree.path.into_os_string(),
                    ];
                    self.confirm(Operation::git(args));
                }
            }
            if key_matches("n", key) {
                self.prompt_for(Prompt::Worktree(String::new()));
            }
            return Ok(());
        }
        if let Some(row) = self
            .selected_branch_row()
            .filter(|row| row.is_group())
            .cloned()
        {
            if key_matches("<enter>", key) {
                let cache = usize::from(self.focus == Panel::RemoteBranches);
                if !self.collapsed_branches[cache].remove(&row.path) {
                    self.collapsed_branches[cache].insert(row.path);
                }
                self.rebuild_branch_rows();
                return Ok(());
            }
            if key_matches("<space>", key) {
                let references: Vec<_> = row
                    .descendants
                    .iter()
                    .map(|index| self.snapshot.references[*index].clone())
                    .collect();
                self.toggle_branch_visibility(&references);
                return Ok(());
            }
        }
        if key_matches("f", key) {
            self.run_operation(Operation::git(["fetch", "--all", "--prune", "--progress"]));
            return Ok(());
        }
        let reference = self.selected_ref().cloned();
        if self.is("universal", "new", &["n"], key) {
            self.prompt_for(Prompt::Branch(
                reference
                    .as_ref()
                    .map(|reference| reference.oid.clone())
                    .unwrap_or_default(),
            ));
        } else if self.is("branches", "checkoutBranchByName", &["c"], key) {
            self.prompt_for(Prompt::CheckoutBranch);
        } else if self.is("branches", "checkoutPreviousBranch", &["-"], key) {
            self.run_operation(Operation::git(["switch", "-"]));
        } else if let Some(reference) = reference {
            if key_matches("<space>", key) {
                self.toggle_branch_visibility(std::slice::from_ref(&reference));
            } else if key_matches("<enter>", key) {
                let operation = match reference.kind {
                    RefKind::Branch => Operation::git(["switch", "--", &reference.name]),
                    RefKind::Remote => Operation::git(["switch", "--track", &reference.full_name]),
                    RefKind::Tag => Operation::git(["switch", "--detach", &reference.oid]),
                };
                self.run_operation(operation);
            } else if self.is("universal", "remove", &["d"], key) {
                let operation = match reference.kind {
                    RefKind::Tag => Operation::git(["tag", "--delete", "--", &reference.name]),
                    RefKind::Branch => {
                        Operation::git(["branch", "--delete", "--", &reference.name])
                    }
                    RefKind::Remote => {
                        Operation::git(["branch", "-r", "--delete", "--", &reference.name])
                    }
                };
                self.confirm(operation);
            } else if self.is("branches", "renameBranch", &["R"], key) {
                self.prompt_for(Prompt::RenameBranch(reference.name));
            } else if self.is("branches", "mergeIntoCurrentBranch", &["M"], key) {
                self.confirm(Operation::git(["merge", &reference.full_name]));
            } else if self.is("branches", "rebaseBranch", &["r"], key) {
                self.confirm(Operation::git(["rebase", &reference.full_name]));
            } else if self.is("branches", "forceCheckoutBranch", &["F"], key) {
                self.confirm(Operation::git([
                    "switch",
                    "--discard-changes",
                    "--",
                    &reference.name,
                ]));
            } else if self.is("branches", "createTag", &["T"], key) {
                self.prompt_for(Prompt::Tag(reference.oid));
            } else if self.is("universal", "newWorktree", &["w"], key) {
                self.prompt_for(Prompt::Worktree(reference.name));
            } else if key_matches("y", key) {
                self.copy(reference.name);
            } else if key_matches("u", key) {
                let branch = self
                    .snapshot
                    .references
                    .iter()
                    .find(|reference| reference.current)
                    .map(|reference| reference.name.clone())
                    .context("No current local branch")?;
                self.run_operation(Operation::git([
                    "branch",
                    "--set-upstream-to",
                    &reference.name,
                    &branch,
                ]));
            } else if key_matches("f", key) {
                self.run_operation(Operation::git(["fetch", "--all", "--prune", "--progress"]));
            }
        }
        Ok(())
    }

    fn commits_key(&mut self, key: KeyEvent) -> Result<()> {
        let Some(commit) = self.selected_commit().cloned() else {
            return Ok(());
        };
        if key_matches("<enter>", key) {
            self.set_focus(self.file_panel());
        } else if self.is("commits", "checkoutCommit", &["<space>"], key) {
            self.confirm(Operation::git(["switch", "--detach", &commit.oid]));
        } else if self.is("commits", "viewResetOptions", &["g"], key) {
            self.reset_menu(&commit.oid);
        } else if self.is("commits", "revertCommit", &["t"], key) {
            self.confirm(Operation::git(["revert", "--no-edit", &commit.oid]));
        } else if self.is("commits", "cherryPickCopy", &["C"], key) {
            if !self.copied_commits.contains(&commit.oid) {
                self.copied_commits.push(commit.oid);
            }
            self.copied_commits.sort_by_key(|oid| {
                std::cmp::Reverse(
                    self.snapshot
                        .commits
                        .iter()
                        .position(|commit| &commit.oid == oid)
                        .unwrap_or(0),
                )
            });
            self.notice = "Commit copied; V cherry-picks copied commits".into();
        } else if self.is("commits", "pasteCommits", &["V"], key) {
            let commits = self.copied_commits.clone();
            if commits.is_empty() {
                bail!("Copy commits with C first");
            }
            let mut args: Vec<OsString> = vec!["cherry-pick".into()];
            args.extend(commits.into_iter().map(Into::into));
            self.confirm(Operation::git(args));
        } else if self.is("commits", "createFixupCommit", &["F"], key) {
            self.run_operation(Operation::git(["commit", "--fixup", &commit.oid]));
        } else if self.is("commits", "createTag", &["T"], key) {
            self.prompt_for(Prompt::Tag(commit.oid));
        } else if self.is("commits", "startInteractiveRebase", &["i"], key) {
            self.pending_external = Some(interactive_rebase(&commit));
        } else if self.is("commits", "renameCommitWithEditor", &["R"], key)
            || self.is("commits", "renameCommit", &["r"], key)
        {
            if commit.oid == self.snapshot.head {
                self.prompt_for(Prompt::Commit(true));
            } else {
                self.pending_external = Some(interactive_rebase(&commit));
            }
        } else if self.is("commits", "copyCommitAttributeToClipboard", &["y"], key) {
            self.copy(commit.oid);
        } else if self.is("commits", "viewBisectOptions", &["b"], key) {
            self.menu(
                "Bisect",
                vec![
                    run_item("Start bisect", ["bisect", "start"], false),
                    run_item(
                        "Mark selected commit good",
                        ["bisect", "good", &commit.oid],
                        false,
                    ),
                    run_item(
                        "Mark selected commit bad",
                        ["bisect", "bad", &commit.oid],
                        false,
                    ),
                    run_item("Skip current commit", ["bisect", "skip"], false),
                    run_item("Finish bisect", ["bisect", "reset"], false),
                ],
            );
        }
        Ok(())
    }

    fn stash_key(&mut self, key: KeyEvent) -> Result<()> {
        if key_matches("s", key) {
            self.prompt_for(Prompt::Stash);
            return Ok(());
        }
        let Some(stash) = self.selected_stash().cloned() else {
            return Ok(());
        };
        if key_matches("<space>", key) {
            self.confirm(Operation::git(["stash", "apply", &stash.reference]));
        } else if self.is("stash", "popStash", &["g"], key) {
            self.confirm(Operation::git(["stash", "pop", &stash.reference]));
        } else if key_matches("d", key) {
            self.confirm(Operation::git(["stash", "drop", &stash.reference]));
        } else if key_matches("<enter>", key) {
            self.set_focus(Panel::Diff);
        }
        Ok(())
    }

    fn reset_menu(&mut self, oid: &str) {
        self.menu(
            "Reset current branch to selected commit",
            vec![
                run_item(
                    "Soft — keep index and working files",
                    ["reset", "--soft", oid],
                    true,
                ),
                run_item(
                    "Mixed — keep working files, reset index",
                    ["reset", "--mixed", oid],
                    true,
                ),
                run_item(
                    "Hard — discard tracked changes",
                    ["reset", "--hard", oid],
                    true,
                ),
            ],
        );
    }

    fn operation_menu(&mut self) {
        self.menu(
            "Operation controls",
            vec![
                MenuItem {
                    label: "Continue rebase (opens editor when needed)".into(),
                    action: MenuAction::External(CommandSpec::git(["rebase", "--continue"])),
                },
                run_item("Abort rebase", ["rebase", "--abort"], true),
                run_item("Skip rebase commit", ["rebase", "--skip"], true),
                MenuItem {
                    label: "Continue merge".into(),
                    action: MenuAction::External(CommandSpec::git(["merge", "--continue"])),
                },
                run_item("Abort merge", ["merge", "--abort"], true),
                run_item("Continue cherry-pick", ["cherry-pick", "--continue"], false),
                run_item("Abort cherry-pick", ["cherry-pick", "--abort"], true),
                run_item("Continue revert", ["revert", "--continue"], false),
                run_item("Abort revert", ["revert", "--abort"], true),
            ],
        );
    }

    fn filter_menu(&mut self) {
        self.menu(
            "Filters",
            vec![
                MenuItem {
                    label: "Filter commit history by path".into(),
                    action: MenuAction::Input(Prompt::HistoryPath),
                },
                MenuItem {
                    label: "Clear history path filter".into(),
                    action: MenuAction::HistoryClear,
                },
                MenuItem {
                    label: "Toggle changed-files-only view".into(),
                    action: MenuAction::ToggleFilter,
                },
                MenuItem {
                    label: "Toggle unchanged tracked files".into(),
                    action: MenuAction::ToggleUnchanged,
                },
            ],
        );
    }
    fn recent_menu(&mut self) {
        let items = self
            .state
            .recent_repos
            .iter()
            .map(|path| MenuItem {
                label: path.display().to_string(),
                action: MenuAction::SwitchRepo(path.clone()),
            })
            .collect();
        self.menu("Recent repositories", items);
    }
    fn pull_menu(&mut self) {
        if self.warn_if_busy("Open pull options") {
            return;
        }
        let current = self
            .snapshot
            .references
            .iter()
            .find(|reference| reference.current);
        let title = current.map_or_else(
            || "Pull · current branch".into(),
            |reference| {
                format!(
                    "Pull · {} ← {}",
                    reference.name,
                    if reference.upstream.is_empty() {
                        "configured remote"
                    } else {
                        &reference.upstream
                    }
                )
            },
        );
        self.menu(
            title,
            vec![
                run_item(
                    "Fast-forward only (stop if branches diverged)",
                    ["pull", "--ff-only", "--progress"],
                    false,
                ),
                run_item(
                    "Merge remote changes (allow fast-forward)",
                    ["pull", "--no-rebase", "--ff", "--progress"],
                    false,
                ),
                run_item(
                    "Rebase local commits onto upstream",
                    ["pull", "--rebase", "--ff", "--progress"],
                    false,
                ),
                run_item(
                    "Rebase + autostash tracked working changes",
                    ["pull", "--rebase", "--ff", "--autostash", "--progress"],
                    false,
                ),
                MenuItem {
                    label: "Overwrite local branch and tracked changes with upstream…".into(),
                    action: MenuAction::Run(crate::git::pull_overwrite_operation(), true),
                },
                run_item(
                    "Fetch only (leave local commits and files as they are)",
                    ["fetch", "--prune", "--progress"],
                    false,
                ),
            ],
        );
    }
    fn push_menu(&mut self, reference: Option<crate::model::Reference>) {
        if self.warn_if_busy("Open push options") {
            return;
        }
        let reference =
            reference.filter(|reference| !reference.current || reference.kind == RefKind::Tag);
        let title = reference.as_ref().map_or_else(
            || {
                format!(
                    "Push · {} → configured remote",
                    self.snapshot
                        .references
                        .iter()
                        .find(|reference| reference.current)
                        .map_or("HEAD", |reference| reference.name.as_str())
                )
            },
            |reference| format!("Push · {} → origin", reference.name),
        );
        let operation = |flag: Option<&str>| {
            let mut args: Vec<OsString> = vec!["push".into(), "--progress".into()];
            if let Some(flag) = flag {
                args.push(flag.into());
            }
            if let Some(reference) = &reference {
                if reference.kind == RefKind::Branch {
                    args.push("--set-upstream".into());
                }
                args.extend(["origin".into(), reference.full_name.clone().into()]);
            }
            Operation::git(args)
        };
        let mut lease = operation(Some("--force-with-lease"));
        lease.label = format!(
            "Replace remote history only if its tip matches the known remote ref: {}",
            lease.label
        );
        let mut force = operation(Some("--force"));
        force.label = format!(
            "Overwrite remote history, including commits made by others: {}",
            force.label
        );
        let mut items = vec![
            MenuItem {
                label: "Push normally".into(),
                action: MenuAction::Run(operation(None), false),
            },
            MenuItem {
                label: "Force push with lease (check known remote tip)…".into(),
                action: MenuAction::Run(lease, true),
            },
            MenuItem {
                label: "Force push (overwrite remote history)…".into(),
                action: MenuAction::Run(force, true),
            },
            MenuItem {
                label: "Dry run (preview without updating remote)".into(),
                action: MenuAction::Run(operation(Some("--dry-run")), false),
            },
        ];
        if reference.is_none() {
            if let Some(current) = self
                .snapshot
                .references
                .iter()
                .find(|reference| reference.current)
            {
                items.push(run_item(
                    "Set upstream and push current branch to origin",
                    [
                        "push",
                        "--progress",
                        "--set-upstream",
                        "origin",
                        &current.full_name,
                    ],
                    false,
                ));
            }
        }
        self.menu(title, items);
    }
    fn repository_menu(&mut self) {
        let mut items = vec![
            run_item(
                "Fetch all remotes",
                ["fetch", "--all", "--prune", "--progress"],
                false,
            ),
            MenuItem {
                label: "Pull options…".into(),
                action: MenuAction::PullMenu,
            },
            MenuItem {
                label: "Push options…".into(),
                action: MenuAction::PushMenu(None),
            },
            run_item(
                "Initialize/update all submodules",
                ["submodule", "update", "--init", "--recursive", "--progress"],
                false,
            ),
            MenuItem {
                label: "Run interactive Git/shell command".into(),
                action: MenuAction::Input(Prompt::Shell(true)),
            },
        ];
        for file in &self.snapshot.files {
            if self.repo.root.join(&file.path).join(".git").exists() {
                items.push(MenuItem {
                    label: format!("Open submodule {}", file.path.display()),
                    action: MenuAction::SwitchRepo(self.repo.root.join(&file.path)),
                });
                let mut command = CommandSpec::git([
                    OsString::from("submodule"),
                    "update".into(),
                    "--init".into(),
                    "--recursive".into(),
                    "--progress".into(),
                    "--".into(),
                    file.path.as_os_str().to_owned(),
                ]);
                command.label = format!("Initialize/update submodule {}", file.path.display());
                items.push(MenuItem {
                    label: command.label.clone(),
                    action: MenuAction::Run(
                        Operation {
                            label: command.label.clone(),
                            steps: vec![Step::Command(command)],
                        },
                        false,
                    ),
                });
                if let Ok(operation) = crate::git::reset_submodule_operation(&file.path) {
                    items.push(MenuItem {
                        label: format!("Stash and reset submodule {}", file.path.display()),
                        action: MenuAction::Run(operation, true),
                    });
                }
            }
        }
        self.menu("Repository", items);
    }

    fn merge_menu(&mut self) -> Result<()> {
        let path = self.row().context("Select a conflicted file")?.path.clone();
        let ours = Operation::git([
            OsString::from("checkout"),
            "--ours".into(),
            "--".into(),
            path.as_os_str().to_owned(),
        ]);
        let theirs = Operation::git([
            OsString::from("checkout"),
            "--theirs".into(),
            "--".into(),
            path.as_os_str().to_owned(),
        ]);
        let tool = CommandSpec::git([
            OsString::from("mergetool"),
            "--no-prompt".into(),
            "--".into(),
            path.as_os_str().to_owned(),
        ]);
        self.menu(
            "Resolve file conflict (stage after resolving)",
            vec![
                MenuItem {
                    label: "Use ours".into(),
                    action: MenuAction::Run(ours, true),
                },
                MenuItem {
                    label: "Use theirs".into(),
                    action: MenuAction::Run(theirs, true),
                },
                MenuItem {
                    label: "Open configured merge tool".into(),
                    action: MenuAction::External(tool),
                },
            ],
        );
        Ok(())
    }

    fn external_tool(&mut self, merge: bool) -> Result<()> {
        if self.side == Panel::HeadFiles {
            return Ok(());
        }
        let row = self.row().context("Select a file first")?;
        let mut args: Vec<OsString> = vec![
            if merge { "mergetool" } else { "difftool" }.into(),
            "--no-prompt".into(),
        ];
        if !merge && self.side == Panel::CommitFiles {
            if let Some(oid) = &self.stash_inspection_oid {
                args.extend([format!("{}^", oid).into(), oid.clone().into()]);
            } else if let Some(info) = &self.comparison {
                args.extend([info.diff_base().into(), info.to_oid.clone().into()]);
            } else {
                args.extend([
                    format!("{}^", self.commit_oid).into(),
                    self.commit_oid.clone().into(),
                ]);
            }
        } else if !merge && self.preview.staged {
            args.push("--cached".into());
        }
        args.extend(["--".into(), row.path.as_os_str().to_owned()]);
        self.pending_external = Some(CommandSpec::git(args));
        Ok(())
    }

    fn edit_file(&mut self) -> Result<()> {
        if self.side == Panel::HeadFiles {
            return Ok(());
        }
        let row = self.row().context("Select a file first")?;
        let path = self.repo.root.join(&row.path);
        if !self.config.os.edit.is_empty() {
            let command = self
                .config
                .os
                .edit
                .replace("{{filename}}", &shell_words::quote(&path.to_string_lossy()))
                .replace("{{line}}", "1");
            self.pending_external = Some(CommandSpec::shell(command));
        } else {
            let editor = env::var("VISUAL")
                .or_else(|_| env::var("EDITOR"))
                .unwrap_or_else(|_| {
                    if cfg!(windows) {
                        "notepad".into()
                    } else {
                        "vi".into()
                    }
                });
            let mut words = shell_words::split(&editor)?.into_iter();
            let program = words.next().context("EDITOR is empty")?;
            let mut args: Vec<OsString> = words.map(Into::into).collect();
            args.push(path.into_os_string());
            self.pending_external = Some(CommandSpec {
                program: program.into(),
                args,
                stdin: None,
                env: Vec::new(),
                label: editor,
            });
        }
        Ok(())
    }
    fn open_file(&mut self) -> Result<()> {
        if self.side == Panel::HeadFiles {
            return Ok(());
        }
        let row = self.row().context("Select a file first")?;
        let path = self.repo.root.join(&row.path);
        if path.join(".git").exists() {
            self.pending_repo = Some(path);
            return Ok(());
        }
        if !self.config.os.open.is_empty() {
            self.pending_external =
                Some(CommandSpec::shell(self.config.os.open.replace(
                    "{{filename}}",
                    &shell_words::quote(&path.to_string_lossy()),
                )));
        } else {
            let program = if cfg!(target_os = "macos") {
                "open"
            } else if cfg!(windows) {
                "explorer"
            } else {
                "xdg-open"
            };
            self.pending_external = Some(CommandSpec {
                program: program.into(),
                args: vec![path.into_os_string()],
                stdin: None,
                env: Vec::new(),
                label: "Open file".into(),
            });
        }
        Ok(())
    }

    fn copy(&mut self, text: String) {
        let program = if cfg!(target_os = "macos") {
            "pbcopy"
        } else if cfg!(windows) {
            "clip"
        } else {
            "xclip"
        };
        let args = if cfg!(all(unix, not(target_os = "macos"))) {
            vec!["-selection".into(), "clipboard".into()]
        } else {
            Vec::new()
        };
        let command = CommandSpec {
            program: program.into(),
            args,
            stdin: Some(text.into_bytes()),
            env: Vec::new(),
            label: "Copy selection to clipboard".into(),
        };
        self.run_operation(Operation {
            label: command.label.clone(),
            steps: vec![Step::Command(command)],
        });
    }

    fn run_custom(&mut self, command: CustomCommand, responses: &[String]) -> Result<()> {
        let mut values = std::collections::BTreeMap::new();
        if let Some(row) = self.row() {
            values.insert(
                ".SelectedFile.Name".to_owned(),
                row.path.to_string_lossy().into_owned(),
            );
            values.insert(
                ".SelectedCommitFile.Name".to_owned(),
                row.path.to_string_lossy().into_owned(),
            );
        }
        if let Some(reference) = self.selected_ref() {
            for key in [
                ".SelectedLocalBranch.Name",
                ".SelectedRemoteBranch.Name",
                ".SelectedTag.Name",
            ] {
                values.insert(key.to_owned(), reference.name.clone());
            }
        }
        if let Some(commit) = self.selected_commit() {
            values.insert(".SelectedCommit.Sha".to_owned(), commit.oid.clone());
        }
        if let Some(stash) = self.selected_stash() {
            values.insert(
                ".SelectedStashEntry.Name".to_owned(),
                stash.reference.clone(),
            );
        }
        for (index, response) in responses.iter().enumerate() {
            values.insert(format!("index .PromptResponses {index}"), response.clone());
            if let Some(key) = command
                .prompts
                .get(index)
                .and_then(|prompt| prompt.get("key"))
                .and_then(|key| key.as_str())
            {
                values.insert(format!(".Form.{key}"), response.clone());
            }
        }
        let rendered = render_template(&command.command, &values)?;
        if command.subprocess {
            self.pending_external = Some(CommandSpec::shell(rendered));
        } else {
            self.run_operation(Operation::shell(rendered));
        }
        Ok(())
    }
}

fn run_item(
    label: &str,
    args: impl IntoIterator<Item = impl Into<OsString>>,
    confirm: bool,
) -> MenuItem {
    MenuItem {
        label: label.to_owned(),
        action: MenuAction::Run(Operation::git(args), confirm),
    }
}

fn interactive_rebase(commit: &crate::model::Commit) -> CommandSpec {
    if commit.parents.is_empty() {
        CommandSpec::git(["rebase", "--interactive", "--root"])
    } else {
        CommandSpec::git(["rebase", "--interactive", &format!("{}^", commit.oid)])
    }
}

pub fn render_template(
    template: &str,
    values: &std::collections::BTreeMap<String, String>,
) -> Result<String> {
    let mut result = String::new();
    let mut rest = template;
    while let Some((prefix, tail)) = rest.split_once("{{") {
        result.push_str(prefix);
        let (expression, tail) = tail
            .split_once("}}")
            .context("Unclosed custom command template")?;
        let expression = expression.trim();
        let (key, quote) = if let Some(key) = expression.strip_prefix("quote ") {
            (key.trim(), true)
        } else if let Some(key) = expression.strip_suffix("| quote") {
            (key.trim(), true)
        } else {
            (expression, false)
        };
        let value = values.get(key).with_context(|| {
            format!("Unsupported or unavailable custom command template: {key}")
        })?;
        if quote {
            result.push_str(&shell_words::quote(value));
        } else {
            result.push_str(value);
        }
        rest = tail;
    }
    result.push_str(rest);
    Ok(result)
}

pub fn strip_ansi(text: &str) -> String {
    let mut chars = text.chars().peekable();
    let mut result = String::with_capacity(text.len());
    while let Some(character) = chars.next() {
        if character != '\u{1b}' {
            if character == '\t' {
                result.push_str("    ");
            } else if !character.is_control() || matches!(character, '\n' | '\r') {
                result.push(character);
            }
            continue;
        }
        match chars.next() {
            Some('[') => {
                for character in chars.by_ref() {
                    if ('@'..='~').contains(&character) {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(character) = chars.next() {
                    if character == '\u{7}' || character == '\u{1b}' && chars.peek() == Some(&'\\')
                    {
                        if character == '\u{1b}' {
                            chars.next();
                        }
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    result
}
