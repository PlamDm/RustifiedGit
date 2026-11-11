use crate::{
    app::App,
    config::{key_event, key_matches},
    model::{Panel, RefKind},
};
use crossterm::event::KeyEvent;
use serde_yaml::Value;

#[derive(Clone, Debug)]
pub struct Action {
    pub key: String,
    pub label: String,
    pub description: String,
    pub event: KeyEvent,
}

pub fn file_opener_context(app: &App) -> bool {
    matches!(app.focus, Panel::Files | Panel::CommitFiles | Panel::Diff)
        || (app.focus == Panel::Commits && app.graph_working_tree)
}

fn in_context(context: &str, app: &App) -> bool {
    context
        .split(',')
        .any(|name| name.trim() == "global" || name.trim() == app.focus.context())
}

pub fn display_key(binding: &str) -> String {
    if !binding.starts_with('<') {
        return binding.to_owned();
    }
    binding
        .trim_matches(['<', '>'])
        .split('+')
        .map(|part| match part {
            "ctrl" => "Ctrl",
            "alt" => "Alt",
            "shift" => "Shift",
            "enter" => "Enter",
            "space" => "Space",
            "up" => "↑",
            "down" => "↓",
            "left" => "←",
            "right" => "→",
            "pgup" => "PgUp",
            "pgdown" => "PgDn",
            "end" => "End",
            "home" => "Home",
            other => other,
        })
        .collect::<Vec<_>>()
        .join("+")
}

pub fn context_key(app: &App) -> Option<String> {
    let opener_taken = app.config.custom_commands.iter().any(|command| {
        in_context(&command.context, app)
            && key_event("o").is_some_and(|event| key_matches(&command.key, event))
    });
    if !file_opener_context(app) && !opener_taken {
        return Some("o".into());
    }
    let mut catalog = Catalog {
        app,
        items: Vec::new(),
    };
    catalog.add(
        "universal",
        "contextActions",
        "O",
        "Actions",
        "Show available commands.",
    );
    catalog.items.into_iter().next().map(|item| item.key)
}

struct Catalog<'a> {
    app: &'a App,
    items: Vec<Action>,
}

impl Catalog<'_> {
    fn add(&mut self, group: &str, name: &str, fallback: &str, label: &str, description: &str) {
        let bindings: Vec<&str> = match self
            .app
            .config
            .keybinding
            .get(group)
            .and_then(|group| group.get(name))
        {
            Some(Value::String(key)) => vec![key],
            Some(Value::Sequence(keys)) => keys.iter().filter_map(Value::as_str).collect(),
            Some(_) => vec![],
            None => vec![fallback],
        };
        for binding in bindings {
            let Some(event) = key_event(binding) else {
                continue;
            };
            if self.app.config.custom_commands.iter().any(|command| {
                in_context(&command.context, self.app) && key_matches(&command.key, event)
            }) {
                continue;
            }
            self.items.push(Action {
                key: display_key(binding),
                label: label.into(),
                description: description.into(),
                event,
            });
            break;
        }
    }
    fn key(&mut self, key: &str, label: &str, description: &str) {
        self.add("", "", key, label, description);
    }

    fn files(&mut self) {
        let app = self.app;
        let selected = app.row().is_some();
        let working =
            app.focus == Panel::Files || (app.focus == Panel::Commits && app.graph_working_tree);
        let writable = app.busy.is_none();
        if app.focus != Panel::Commits && selected {
            self.key(
                "<enter>",
                if app.row().is_some_and(|row| row.directory) {
                    "Expand/collapse directory"
                } else {
                    "Inspect full screen"
                },
                "Open the selected file full screen, or toggle the directory.",
            );
        }
        if working {
            self.add(
                "files",
                "refreshFiles",
                "r",
                "Refresh files",
                "Refresh working-file status without reloading unchanged history.",
            );
            if writable {
                self.add(
                    "files",
                    "fetch",
                    "f",
                    "Fetch all remotes",
                    "Fetch remote updates and prune removed remote references.",
                );
            }
            if writable && selected && app.row().is_some_and(|row| row.changed && !row.ignored) {
                self.add(
                    "universal",
                    "select",
                    "<space>",
                    "Stage / unstage",
                    "Toggle the selected file or directory in the index.",
                );
                self.add(
                    "universal",
                    "remove",
                    "d",
                    "Discard changes…",
                    "Confirm discarding changes in the selected file or directory.",
                );
            }
            if writable && app.changed_file_count() > 0 {
                self.add(
                    "files",
                    "toggleStagedAll",
                    "a",
                    "Stage all",
                    "Stage all changed files, excluding ignored files.",
                );
                self.add(
                    "files",
                    "stashAllChanges",
                    "s",
                    "Save stash…",
                    "Save working changes, including untracked files, with a message.",
                );
                self.add(
                    "files",
                    "viewStashOptions",
                    "S",
                    "Stash options…",
                    "Choose tracked-only, staged-only or keep-index stashing.",
                );
            }
            if writable && app.staged_file_count() > 0 {
                self.add(
                    "files",
                    "commitChanges",
                    "c",
                    "Commit…",
                    "Enter a message and commit the staged changes.",
                );
                self.add(
                    "files",
                    "commitChangesWithEditor",
                    "C",
                    "Commit in editor",
                    "Open Git's configured commit-message editor.",
                );
                self.add(
                    "files",
                    "commitChangesWithoutHook",
                    "w",
                    "Commit without hooks",
                    "Open the commit editor with repository hooks disabled.",
                );
            }
            if writable && !app.snapshot.head.is_empty() {
                self.add(
                    "files",
                    "amendLastCommit",
                    "A",
                    "Amend…",
                    "Confirm changing the most recent commit.",
                );
                self.add(
                    "files",
                    "viewResetOptions",
                    "D",
                    "Reset / clean options…",
                    "Choose working-tree, index or submodule reset actions.",
                );
            }
            self.add(
                "files",
                "toggleIgnoredFiles",
                "I",
                "Show / hide ignored",
                "Include or hide ignored paths in the working-file tree.",
            );
            self.add(
                "files",
                "toggleTreeView",
                "\u{60}",
                "Tree / flat view",
                "Switch the working-file list between a tree and flat paths.",
            );
            self.add(
                "files",
                "collapseAll",
                "-",
                "Collapse directories",
                "Collapse every directory in the working-file tree.",
            );
            self.add(
                "files",
                "expandAll",
                "=",
                "Expand directories",
                "Expand every directory in the working-file tree.",
            );
            if selected {
                self.key(
                    "v",
                    "Mark / unmark",
                    "Toggle the selected path in the multiple-file selection.",
                );
                if writable {
                    self.add(
                        "files",
                        "ignoreFile",
                        "i",
                        "Ignore path…",
                        "Confirm adding the selected path to .gitignore.",
                    );
                }
                self.add(
                    "files",
                    "copyFileInfoToClipboard",
                    "y",
                    "Copy path",
                    "Copy the selected repository-relative path.",
                );
            }
        } else if app.focus == Panel::CommitFiles {
            self.key(
                "\u{60}",
                "Tree / flat view",
                "Switch the committed-file list between a tree and flat paths.",
            );
            if selected && writable && app.comparison.is_none() && !app.stash_inspection_active() {
                self.add(
                    "commitFiles",
                    "cherryPickFiles",
                    "V",
                    "Cherry-pick selected files…",
                    "Apply only the selected commit-file changes to the working tree and index, without committing.",
                );
                self.add(
                    "universal",
                    "remove",
                    "d",
                    "Discard selected working changes…",
                    "Confirm discarding current uncommitted changes only for the selected commit paths.",
                );
                self.add(
                    "commitFiles",
                    "checkoutCommitFile",
                    "c",
                    "Restore committed file…",
                    "Confirm restoring this commit's version into the working tree.",
                );
            }
            if !app.stash_inspection_active() {
                self.key(
                    "v",
                    "Mark / unmark",
                    "Toggle this path in the multiple-file selection.",
                );
            }
            self.add(
                "files",
                "collapseAll",
                "-",
                "Collapse directories",
                "Collapse all commit-file directories.",
            );
            self.add(
                "files",
                "expandAll",
                "=",
                "Expand directories",
                "Expand all commit-file directories.",
            );
        }
        if selected {
            if app.focus == Panel::CommitFiles {
                self.key(
                    "e",
                    "Edit file",
                    "Open the selected working file in the configured editor.",
                );
                self.key(
                    "o",
                    "Open externally",
                    "Open the selected working path with the system opener.",
                );
            } else {
                self.add(
                    "universal",
                    "edit",
                    "e",
                    "Edit file",
                    "Open the selected working file in the configured editor.",
                );
                self.add(
                    "universal",
                    "openFile",
                    "o",
                    "Open externally",
                    "Open the selected working path with the configured system opener.",
                );
            }
            self.add(
                "universal",
                "openDiffTool",
                "<ctrl+t>",
                "External diff tool",
                "Inspect the selection with Git's configured diff tool.",
            );
            if working && app.has_conflicted_files() {
                self.add(
                    "files",
                    "openMergeToolForFile",
                    "<alt+m>",
                    "External merge tool",
                    "Open Git's configured merge tool for the selected path.",
                );
                if writable {
                    self.add(
                        "files",
                        "openMergeOptions",
                        "M",
                        "Conflict actions…",
                        "Choose ours/theirs or stage a resolved conflict.",
                    );
                }
            }
            if app.focus != Panel::Commits {
                self.add(
                    "universal",
                    "viewFileHistory",
                    "F",
                    "History of this path",
                    "Show commits touching this file or directory; follow file renames.",
                );
            }
        }
    }
}

pub fn for_app(app: &App) -> Vec<Action> {
    let mut catalog = Catalog {
        app,
        items: Vec::new(),
    };
    let writable = app.busy.is_none();
    catalog.add(
        "universal", "compareRevisions", "B",
        if app.comparison.is_some() { "Comparison options…" } else { "Compare revisions…" },
        "Mark a commit or branch as A, then compare it with another selection; compare two names or compare a branch with its common ancestor.",
    );
    match app.focus {
        Panel::Commits if app.graph_working_tree => {
            catalog.key(
                "<enter>",
                "Inspect working files",
                "Focus the changed files in the working tree / index.",
            );
            catalog.files();
        }
        Panel::Commits => {
            if app.selected_commit().is_some() {
                catalog.key(
                    "<enter>",
                    "Inspect changed files",
                    "Focus the files changed by the selected commit.",
                );
                catalog.add(
                    "commits",
                    "cherryPickCopy",
                    "C",
                    "Copy commit",
                    "Add this commit to the cherry-pick selection.",
                );
                catalog.add(
                    "commits",
                    "copyCommitAttributeToClipboard",
                    "y",
                    "Copy hash",
                    "Copy the full selected commit ID.",
                );
                if writable {
                    catalog.add(
                        "commits",
                        "checkoutCommit",
                        "<space>",
                        "Checkout commit…",
                        "Confirm switching to this commit with a detached HEAD.",
                    );
                    catalog.add(
                        "commits",
                        "viewResetOptions",
                        "g",
                        "Reset options…",
                        "Choose how to reset the current branch to this commit.",
                    );
                    catalog.add(
                        "commits",
                        "revertCommit",
                        "t",
                        "Revert commit…",
                        "Confirm creating a commit that reverses this commit.",
                    );
                    catalog.add(
                        "commits",
                        "createTag",
                        "T",
                        "Create tag…",
                        "Name a new tag pointing at the selected commit.",
                    );
                    catalog.add(
                        "commits",
                        "startInteractiveRebase",
                        "i",
                        "Interactive rebase",
                        "Open the rebase editor starting at this commit.",
                    );
                    catalog.add(
                        "commits",
                        "renameCommit",
                        "r",
                        "Reword / amend…",
                        "Edit this commit's message, using rebase for older commits.",
                    );
                    catalog.add(
                        "commits",
                        "renameCommitWithEditor",
                        "R",
                        "Reword / rebase…",
                        "Amend the HEAD message or open the rebase editor for an older commit.",
                    );
                    catalog.add(
                        "commits",
                        "viewBisectOptions",
                        "b",
                        "Bisect options…",
                        "Start, mark, skip or finish a Git bisect.",
                    );
                    if app.staged_file_count() > 0 {
                        catalog.add(
                            "commits",
                            "createFixupCommit",
                            "F",
                            "Create fixup commit",
                            "Commit the staged changes as a fixup for this commit.",
                        );
                    }
                    if !app.copied_commits.is_empty() {
                        catalog.add(
                            "commits",
                            "pasteCommits",
                            "V",
                            "Cherry-pick copied commits…",
                            "Confirm applying the copied commits in source order.",
                        );
                    }
                }
            }
        }
        Panel::Branches | Panel::RemoteBranches => {
            if app.focus == Panel::Branches && app.branch_tab == 3 {
                if !app.snapshot.worktrees.is_empty() {
                    catalog.key(
                        "<enter>",
                        "Open worktree",
                        "Switch the app to the selected worktree.",
                    );
                    if writable {
                        catalog.key(
                            "d",
                            "Remove worktree…",
                            "Confirm removing the selected worktree.",
                        );
                    }
                }
                if writable {
                    catalog.key("n", "New worktree…", "Enter the path for a new worktree.");
                }
            } else {
                let group = app
                    .branch_rows(app.focus == Panel::RemoteBranches)
                    .get(app.selected(app.focus))
                    .is_some_and(|row| row.is_group());
                if group {
                    catalog.key(
                        "<enter>",
                        "Expand / collapse group",
                        "Toggle this branch group and its nested groups.",
                    );
                    catalog.key(
                        "<space>",
                        "Toggle group in graph",
                        "Enable all branches in this group, or disable them when all are enabled.",
                    );
                }
                if let Some(reference) = app.selected_ref() {
                    catalog.key(
                        "<space>",
                        "Toggle branch in graph",
                        "Show or hide this branch without checking it out.",
                    );
                    catalog.key("y", "Copy ref name", "Copy the full branch or tag name.");
                    if writable {
                        catalog.key(
                            "<enter>",
                            "Checkout reference",
                            "Switch to the selected branch, remote branch or tag.",
                        );
                        catalog.add(
                            "universal",
                            "remove",
                            "d",
                            "Delete reference…",
                            "Confirm deleting the selected local reference.",
                        );
                        if reference.kind == RefKind::Branch {
                            catalog.add(
                                "branches",
                                "renameBranch",
                                "R",
                                "Rename branch…",
                                "Enter a new name for the selected local branch.",
                            );
                            catalog.add(
                                "branches",
                                "forceCheckoutBranch",
                                "F",
                                "Force checkout…",
                                "Confirm checking out the branch and discarding tracked changes.",
                            );
                        }
                        catalog.add(
                            "branches",
                            "mergeIntoCurrentBranch",
                            "M",
                            "Merge reference…",
                            "Confirm merging this reference into the current branch.",
                        );
                        catalog.add(
                            "branches",
                            "rebaseBranch",
                            "r",
                            "Rebase onto reference…",
                            "Confirm rebasing the current branch onto this reference.",
                        );
                        catalog.add(
                            "branches",
                            "createTag",
                            "T",
                            "Tag reference…",
                            "Name a tag at the selected reference's commit.",
                        );
                        catalog.add(
                            "universal",
                            "newWorktree",
                            "w",
                            "Worktree from reference…",
                            "Enter a new worktree path based on this reference.",
                        );
                        if reference.kind != RefKind::Tag {
                            catalog.key(
                                "u",
                                "Set upstream",
                                "Use the selected branch as the current branch's upstream.",
                            );
                        }
                    }
                }
                if writable {
                    catalog.add("universal", "new", "n", "Create branch…", "Create a branch from the selection, or from HEAD when a group is selected.");
                    catalog.add(
                        "branches",
                        "checkoutBranchByName",
                        "c",
                        "Checkout by name…",
                        "Enter the name of a branch to check out.",
                    );
                    catalog.add(
                        "branches",
                        "checkoutPreviousBranch",
                        "-",
                        "Previous branch",
                        "Switch back to the previous checked-out branch.",
                    );
                    catalog.key(
                        "f",
                        "Fetch all remotes",
                        "Fetch remote updates and prune removed remote references.",
                    );
                }
                catalog.key(
                    "<left>",
                    "Collapse / parent group",
                    "Collapse this group or move to its parent.",
                );
                catalog.key("<right>", "Expand group", "Expand the selected group.");
            }
        }
        Panel::Files | Panel::CommitFiles => catalog.files(),
        Panel::HeadFiles => {
            if app.row().is_some() {
                catalog.key(
                    "<enter>",
                    "Inspect HEAD file",
                    "Open committed HEAD contents full screen, or expand the selected directory.",
                );
                catalog.add(
                    "universal",
                    "viewFileHistory",
                    "F",
                    "History of this path",
                    "Show commits touching this HEAD path; follow file renames.",
                );
                catalog.add(
                    "files",
                    "copyFileInfoToClipboard",
                    "y",
                    "Copy path",
                    "Copy the selected repository-relative path.",
                );
            }
            catalog.add(
                "files",
                "toggleTreeView",
                "`",
                "Tree / flat view",
                "Switch HEAD files between a directory tree and flat paths.",
            );
            catalog.add(
                "files",
                "collapseAll",
                "-",
                "Collapse directories",
                "Collapse all visible HEAD directories.",
            );
            catalog.add(
                "files",
                "expandAll",
                "=",
                "Expand directories",
                "Expand all HEAD directories.",
            );
        }
        Panel::Diff if app.side == Panel::HeadFiles => {
            catalog.key(
                "n",
                "Next HEAD file",
                "Inspect the next path in the HEAD file tree.",
            );
            catalog.key(
                "N",
                "Previous HEAD file",
                "Inspect the previous path in the HEAD file tree.",
            );
            catalog.key(
                "H",
                "Scroll left",
                "Reveal earlier columns in the committed file.",
            );
            catalog.key(
                "L",
                "Scroll right",
                "Reveal later columns in the committed file.",
            );
            catalog.add(
                "universal",
                "viewFileHistory",
                "F",
                "History of this path",
                "Show commits touching this HEAD path.",
            );
        }
        Panel::Diff => {
            catalog.key(
                "a",
                "Hunk / line selection",
                "Choose whether Space applies the whole hunk or the selected changed line.",
            );
            if writable && app.side == Panel::Files && !app.patch.hunks.is_empty() {
                catalog.key(
                    "<space>",
                    "Stage / unstage selection",
                    "Apply the selected hunk or line to the index.",
                );
                if !app.preview.staged {
                    catalog.key(
                        "d",
                        "Discard selection…",
                        "Confirm discarding the selected hunk or line.",
                    );
                }
            }
            catalog.key(
                "<left>",
                "Previous hunk",
                "Jump to the preceding diff hunk.",
            );
            catalog.key("<right>", "Next hunk", "Jump to the following diff hunk.");
            catalog.add(
                "universal",
                "increaseContextInDiffView",
                "}",
                "More context",
                "Increase the unchanged context shown around each diff hunk.",
            );
            catalog.add(
                "universal",
                "decreaseContextInDiffView",
                "{",
                "Less context",
                "Decrease unchanged context around diff hunks.",
            );
            catalog.add(
                "universal",
                "toggleWhitespaceInDiffView",
                "<ctrl+w>",
                "Toggle whitespace filter",
                "Show or ignore whitespace-only differences.",
            );
            catalog.key(
                "H",
                "Scroll left",
                "Reveal earlier columns in long diff lines.",
            );
            catalog.key(
                "L",
                "Scroll right",
                "Reveal later columns in long diff lines.",
            );
            if matches!(app.side, Panel::Files | Panel::CommitFiles) && app.row().is_some() {
                catalog.key(
                    "e",
                    "Edit file",
                    "Open the selected working file in the configured editor.",
                );
                catalog.key(
                    "o",
                    "Open externally",
                    "Open the selected working file with the system opener.",
                );
                catalog.add(
                    "universal",
                    "openDiffTool",
                    "<ctrl+t>",
                    "External diff tool",
                    "Open the configured external diff tool.",
                );
                catalog.add(
                    "universal",
                    "viewFileHistory",
                    "F",
                    "History of this path",
                    "Show commits touching the currently selected path.",
                );
            }
            if app.search(Panel::Diff).is_empty() {
                catalog.key(
                    "n",
                    "Next file",
                    "Inspect the next file in the current file list.",
                );
                catalog.key(
                    "N",
                    "Previous file",
                    "Inspect the previous file in the current file list.",
                );
            }
        }
        Panel::Stash => {
            if app.selected_stash().is_some() {
                catalog.key(
                    "<enter>",
                    "Inspect stash",
                    "Focus the selected stash's diff.",
                );
                if writable {
                    catalog.key(
                        "<space>",
                        "Apply stash…",
                        "Confirm applying the stash while keeping it saved.",
                    );
                    catalog.add(
                        "stash",
                        "popStash",
                        "g",
                        "Pop stash…",
                        "Confirm applying the stash and removing it from the stash list.",
                    );
                    catalog.key("d", "Drop stash…", "Confirm removing the selected stash.");
                }
            }
            if writable {
                catalog.key(
                    "s",
                    "Save stash…",
                    "Save local changes, including untracked files.",
                );
            }
        }
        Panel::Status => {
            if app.repo_tab == 0 {
                catalog.key(
                    "<enter>",
                    "Open repository",
                    "Switch to the selected recent repository.",
                );
            } else {
                if let Some(module) = app.selected_submodule() {
                    if module.initialized {
                        catalog.key(
                            "<enter>",
                            "Open submodule",
                            "Open this submodule's graph, branches and working files.",
                        );
                    } else if writable {
                        catalog.key("<enter>", "Initialize submodule…", "Initialize this submodule and its nested submodules, then press Enter to open it.");
                    }
                    if writable {
                        catalog.key("i", "Initialize/update submodule…", "Update the selected submodule and nested submodules to their recorded commits.");
                    }
                    catalog.key(
                        "y",
                        "Copy submodule path",
                        "Copy the full local path to this submodule.",
                    );
                } else if app.repo.superproject.is_some() {
                    catalog.key(
                        "<enter>",
                        "Open parent repository",
                        "Return to the repository containing this submodule.",
                    );
                }
                if writable && !app.snapshot.submodules.is_empty() {
                    catalog.key(
                        "a",
                        "Initialize/update all submodules…",
                        "Initialize or update every submodule recursively to its recorded commit.",
                    );
                }
            }
            catalog.key(
                "r",
                "Repository actions…",
                "Open repository-level fetch, pull, push and maintenance actions.",
            );
        }
        Panel::Details => {
            catalog.key(
                "<enter>",
                "Inspect changed files",
                "Move from commit details to its changed-file list.",
            );
            catalog.key(
                "5",
                "Back to graph",
                "Focus the commit graph while keeping this commit selected.",
            );
        }
        Panel::Output => {
            catalog.key(
                "<end>",
                "Follow output",
                "Jump to the latest output and follow newly arriving lines.",
            );
            catalog.add(
                "universal",
                "extrasMenu",
                "@",
                "Details / output…",
                "Switch the Details and Output tabs in pane 6.",
            );
        }
    }
    if app.focus == Panel::Commits {
        catalog.add(
            "commits", "setHistoryLimit", "L", "Loaded commits…",
            &format!("Choose initial/page size (currently {}); {} commits loaded, target {}. Older commits still load automatically near the end.",
                app.history_page_size(), app.snapshot.commits.len(), app.options.commit_limit),
        );
        if !app.history_exhausted && app.history_error.is_none() {
            catalog.key(
                ".",
                "Load older commits",
                &format!(
                    "Load another {} commits while keeping the selection.",
                    app.history_page_size()
                ),
            );
        }
        if writable {
            catalog.key(
                "G",
                "Accelerate history",
                "Build Git's commit-graph metadata cache for faster history queries.",
            );
        }
    }
    if !(matches!(app.focus, Panel::Status | Panel::Details)
        || app.focus == Panel::Branches && app.branch_tab == 3)
    {
        catalog.add(
            "universal",
            "startSearch",
            "/",
            if matches!(app.focus, Panel::Commits | Panel::Files | Panel::CommitFiles | Panel::HeadFiles) { "Fuzzy search" } else { "Search" },
            "Filter as you type; space combines terms, 'term matches literally, !term excludes. Commit search covers loaded history.",
        );
    }
    if matches!(
        app.focus,
        Panel::Commits | Panel::Files | Panel::CommitFiles | Panel::HeadFiles
    ) {
        catalog.add(
            "universal",
            "fuzzySearch",
            "<ctrl+f>",
            "Fuzzy search",
            "Search this list interactively; arrows select, Enter keeps the filter, Esc cancels.",
        );
    }
    if matches!(app.focus, Panel::Diff | Panel::Output) && !app.search(app.focus).is_empty() {
        catalog.add(
            "universal",
            "nextMatch",
            "n",
            "Next search match",
            "Find the next matching line in the current view.",
        );
        catalog.add(
            "universal",
            "prevMatch",
            "N",
            "Previous search match",
            "Find the previous matching line.",
        );
    }
    catalog.add(
        "universal",
        "filteringMenu",
        "<ctrl+s>",
        "Filters…",
        "Set a history path filter or adjust working-file visibility.",
    );
    if matches!(
        app.focus,
        Panel::Branches
            | Panel::Status
            | Panel::Commits
            | Panel::Diff
            | Panel::Files
            | Panel::CommitFiles
            | Panel::HeadFiles
            | Panel::Details
            | Panel::Output
    ) && !(app.focus == Panel::Diff && app.side == Panel::HeadFiles)
    {
        catalog.add(
            "universal",
            "nextTab",
            "]",
            "Next tab / diff side",
            "Switch Repositories / Submodules, Details / Output, Changes / HEAD tree, branch/history tabs, or staged / unstaged diffs.",
        );
        catalog.add(
            "universal",
            "prevTab",
            "[",
            "Previous tab / diff side",
            "Switch to the previous tab, or toggle staged and unstaged differences.",
        );
    }
    catalog.add(
        "universal",
        "nextScreenMode",
        "+",
        "Zoom pane",
        "Toggle the current pane between the layout and full screen.",
    );
    catalog.add(
        "universal",
        "scrollDownMain",
        "<pgdown>",
        "Next page",
        "Move down by one visible page.",
    );
    catalog.add(
        "universal",
        "scrollUpMain",
        "<pgup>",
        "Previous page",
        "Move up by one visible page.",
    );
    if app.focus == Panel::Commits {
        catalog.key(
            "<shift+down>",
            "Fast down",
            "Move down by one visible page of commits.",
        );
        catalog.key(
            "<shift+up>",
            "Fast up",
            "Move up by one visible page of commits.",
        );
    }
    if matches!(
        app.focus,
        Panel::Files | Panel::CommitFiles | Panel::HeadFiles
    ) {
        catalog.add(
            "files",
            "toggleAllDirectories",
            "*",
            "Expand / collapse all",
            "Toggle all directories in this file tree, including nested folders.",
        );
    }
    if matches!(app.focus, Panel::Files | Panel::CommitFiles) && app.file_selection_enabled() {
        catalog.add(
            "files",
            "deselectFile",
            "u",
            "Deselect file / folder",
            "Remove the hovered file or all files in this folder from the selection.",
        );
        catalog.add("files", "clearSelection", "U", "Deselect all", "Clear every selected file while keeping focus in this pane. Esc also clears the selection.");
        catalog.add("files", "selectAll", "<ctrl+a>", "Select all matching files", "Select every file matching the active filter, including files inside collapsed folders.");
        catalog.key(
            "<shift+down>",
            "Extend selection down",
            "Extend the selection from its anchor. Reverse direction to shrink the range.",
        );
        catalog.key(
            "<shift+up>",
            "Extend selection up",
            "Extend the selection upward from its anchor. Reverse direction to shrink the range.",
        );
        catalog.key(
            "<esc>",
            "Clear selection / return",
            "Clear marked paths before leaving the file list.",
        );
    }
    catalog.add(
        "universal",
        "toggleDiffPreview",
        "~",
        "Automatic preview",
        "Toggle automatic file previews; Enter or 0 still opens a full preview.",
    );
    catalog.add(
        "universal",
        "openRecentRepos",
        "<ctrl+r>",
        "Recent repositories…",
        "Choose another recent repository.",
    );
    if writable {
        catalog.add(
            "universal",
            "pull",
            "p",
            "Pull options…",
            "Choose fast-forward, merge, rebase, autostash, or confirmed overwrite of local changes and commits.",
        );
        catalog.add(
            "universal",
            "push",
            "P",
            "Push options…",
            "Choose normal push, dry run, force with lease, or confirmed force push for the current or selected local reference.",
        );
        catalog.add(
            "universal",
            "executeShellCommand",
            ":",
            "Run command…",
            "Run a command with live output in pane 6's Output tab.",
        );
        catalog.key(
            "!",
            "Interactive command…",
            "Restore the terminal and run an interactive command or editor.",
        );
    } else {
        catalog.key(
            "<ctrl+c>",
            "Cancel command",
            "Cancel the active command and its child processes.",
        );
    }
    catalog.add(
        "universal",
        "createRebaseOptionsMenu",
        "m",
        "Operation controls…",
        "Continue, skip or abort a merge, rebase, cherry-pick or revert.",
    );
    if !matches!(
        app.focus,
        Panel::Branches | Panel::RemoteBranches | Panel::Commits
    ) {
        catalog.add(
            "universal",
            "refresh",
            "R",
            "Refresh",
            "Refresh working files and check whether repository references changed.",
        );
    }
    catalog.add(
        "universal",
        "optionMenu",
        "?",
        "Keyboard help",
        "Show the complete keyboard reference.",
    );
    for command in &app.config.custom_commands {
        if !in_context(&command.context, app) {
            continue;
        }
        if let Some(event) = key_event(&command.key) {
            catalog.items.push(Action {
                key: display_key(&command.key),
                label: if command.description.is_empty() {
                    "Custom command".into()
                } else {
                    command.description.clone()
                },
                description: if command.loading_text.is_empty() {
                    command.command.clone()
                } else {
                    command.loading_text.clone()
                },
                event,
            });
        }
    }
    catalog.items
}
