use crate::{
    actions,
    app::App,
    config::{Config, key_event},
    fuzzy::Query,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_yaml::Value;

#[derive(Clone, Debug)]
pub struct Entry {
    pub section: String,
    pub keys: String,
    pub label: String,
    pub description: String,
}

impl Entry {
    pub fn search_text(&self) -> String {
        format!(
            "{} {} {} {}",
            self.keys, self.label, self.section, self.description
        )
    }
}

pub struct HelpState {
    pub entries: Vec<Entry>,
    pub query: String,
    pub selected: usize,
    pub page_size: usize,
}

impl HelpState {
    pub fn new(app: &App) -> Self {
        let mut entries: Vec<_> = actions::for_app(app)
            .into_iter()
            .map(|action| Entry {
                section: "Current pane".into(),
                keys: action.key,
                label: action.label,
                description: action.description,
            })
            .collect();
        for &(section, group, name, fallback, label, description) in COMMANDS {
            let keys = binding_keys(&app.config, group, name, fallback);
            if entries
                .iter()
                .any(|entry| entry.keys == keys && entry.label == label)
            {
                continue;
            }
            entries.push(Entry {
                section: section.into(),
                keys,
                label: label.into(),
                description: description.into(),
            });
        }
        for command in &app.config.custom_commands {
            let keys = actions::display_key(&command.key);
            let label = if command.description.is_empty() {
                "Custom command"
            } else {
                &command.description
            };
            if entries
                .iter()
                .any(|entry| entry.keys == keys && entry.label == label)
            {
                continue;
            }
            entries.push(Entry {
                section: format!("Custom: {}", command.context),
                keys,
                label: label.into(),
                description: command.command.clone(),
            });
        }
        Self {
            entries,
            query: String::new(),
            selected: 0,
            page_size: 10,
        }
    }

    pub fn matches(&self) -> Vec<usize> {
        if self.query.is_empty() {
            return (0..self.entries.len()).collect();
        }
        let query = Query::new(&self.query);
        let mut scratch = crate::fuzzy::ScoreScratch::default();
        let mut matches: Vec<_> = self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                query
                    .score_with_scratch(&entry.search_text(), &mut scratch)
                    .map(|score| (index, score))
            })
            .collect();
        if !self.query.is_empty() {
            matches.sort_by_key(|(_, score)| std::cmp::Reverse(*score));
        }
        matches.into_iter().map(|(index, _)| index).collect()
    }

    pub fn paste(&mut self, text: &str) {
        self.query.push_str(&text.replace(['\r', '\n'], " "));
        self.selected = 0;
    }

    /// Help consumes text immediately; shortcuts never execute repository actions.
    pub fn key(&mut self, key: KeyEvent) -> bool {
        let last = self.matches().len().saturating_sub(1);
        let page = self.page_size.saturating_sub(1).max(1);
        match key.code {
            KeyCode::Esc => return true,
            KeyCode::Down => self.selected = self.selected.saturating_add(1).min(last),
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::PageDown => self.selected = self.selected.saturating_add(page).min(last),
            KeyCode::PageUp => self.selected = self.selected.saturating_sub(page),
            KeyCode::Home => self.selected = 0,
            KeyCode::End => self.selected = last,
            KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.selected = self.selected.saturating_add(1).min(last);
            }
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.selected = self.selected.saturating_sub(1);
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.query.clear();
                self.selected = 0;
            }
            KeyCode::Backspace | KeyCode::Char('w')
                if key.code == KeyCode::Backspace
                    || key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                if key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                {
                    while self.query.ends_with(char::is_whitespace) {
                        self.query.pop();
                    }
                    while !self.query.is_empty() && !self.query.ends_with(char::is_whitespace) {
                        self.query.pop();
                    }
                } else {
                    self.query.pop();
                }
                self.selected = 0;
            }
            KeyCode::Char(character)
                if !key.modifiers.intersects(
                    KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
                ) =>
            {
                self.query.push(character);
                self.selected = 0;
            }
            _ => {}
        }
        false
    }
}

fn binding_keys(config: &Config, group: &str, name: &str, fallback: &str) -> String {
    if group.is_empty() {
        if let Some((prefix, suffix)) = fallback.split_once(" → ") {
            let action = match prefix {
                "p" => Some("pull"),
                "P" => Some("push"),
                "B" => Some("compareRevisions"),
                _ => None,
            };
            if let Some(action) = action {
                return format!(
                    "{} → {suffix}",
                    binding_keys(config, "universal", action, prefix)
                );
            }
        }
    }
    let keys: Vec<_> = match config
        .keybinding
        .get(group)
        .and_then(|group| group.get(name))
    {
        Some(Value::String(key)) => vec![key.as_str()],
        Some(Value::Sequence(keys)) => keys.iter().filter_map(Value::as_str).collect(),
        Some(_) => Vec::new(),
        None => fallback.split('|').collect(),
    };
    let keys: Vec<_> = keys
        .into_iter()
        .filter(|key| !key.is_empty() && (group.is_empty() || key_event(key).is_some()))
        .map(actions::display_key)
        .collect();
    if keys.is_empty() {
        "Disabled".into()
    } else {
        keys.join(" / ")
    }
}

// section, configuration group/action, default shortcut, command, explanation.
type Spec = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
);
#[rustfmt::skip]
const COMMANDS: &[Spec] = &[
    ("Navigation", "", "", "1", "Repositories", "Focus pane 1. Enter opens the selected recent repository; r opens repository operations."),
    ("Navigation", "", "", "2", "Local branches", "Focus local branches, tags and worktrees in pane 2."),
    ("Navigation", "", "", "3", "Remote branches", "Focus remote branches in pane 3."),
    ("Navigation", "", "", "4", "Stashes", "Focus stashes in pane 4. Hovering a stash loads its changed files into pane 7."),
    ("Navigation", "", "", "5", "Commit graph", "Focus pane 5. Working tree / Index sits above the current branch tip; the active branch is bold."),
    ("Navigation", "", "", "6", "Details / Output", "Focus the active tab of pane 6: commit details or live command output."),
    ("Navigation", "", "", "7", "Changed files", "Focus files for the hovered commit, selected stash, working tree or comparison."),
    ("Navigation", "universal", "focusMainView", "0", "Inspect diff", "Open the selected diff explicitly, including large files and when automatic previews are disabled."),
    ("Navigation", "", "", "8", "Command output", "Open the Output tab in pane 6 directly."),
    ("Navigation", "", "", "Tab / Shift+Tab", "Cycle panes", "Move focus forward or backward through the numbered panes."),
    ("Navigation", "universal", "nextItem", "<down>", "Next item", "Move down one row, or scroll within the current diff hunk."),
    ("Navigation", "universal", "nextItem-alt", "j", "Next item (alternate)", "Move down using the configured alternate navigation key."),
    ("Navigation", "universal", "prevItem", "<up>", "Previous item", "Move up one row, or scroll within the current diff hunk."),
    ("Navigation", "universal", "prevItem-alt", "k", "Previous item (alternate)", "Move up using the configured alternate navigation key."),
    ("Navigation", "universal", "nextBlock", "<right>", "Next pane / expand / next hunk", "Expand a branch group or folder, jump to the next diff hunk, or move to the next pane."),
    ("Navigation", "universal", "prevBlock", "<left>", "Previous pane / collapse / previous hunk", "Collapse a branch group or folder, jump to the previous diff hunk, or move to the previous pane."),
    ("Navigation", "universal", "nextBlock-alt", "l", "Next pane (alternate)", "Alternate for Right: expand, next hunk, or next pane."),
    ("Navigation", "universal", "prevBlock-alt", "h", "Previous pane (alternate)", "Alternate for Left: collapse, previous hunk, or previous pane."),
    ("Navigation", "universal", "scrollDownMain", "<pgdown>", "Next page", "Move down by a visible page; Ctrl+d also pages down."),
    ("Navigation", "universal", "scrollUpMain", "<pgup>", "Previous page", "Move up by a visible page; Ctrl+u also pages up outside text inputs."),
    ("Navigation", "", "", "Home / <", "First item", "Jump to the beginning of the focused list or view."),
    ("Navigation", "", "", "End / >", "Last item", "Jump to the end; in Output, follow newly arriving lines."),
    ("Navigation", "universal", "nextTab", "]", "Next tab / diff side", "Switch branch or history tabs, Changes / HEAD tree, Details / Output, or staged / unstaged diff."),
    ("Navigation", "universal", "prevTab", "[", "Previous tab / diff side", "Switch to the preceding tab or toggle the diff side."),
    ("Navigation", "universal", "nextScreenMode", "+", "Zoom pane", "Toggle the focused pane between the normal layout and full screen."),
    ("Navigation", "universal", "return", "<esc>", "Clear selection / return", "Clear marked paths or filters, leave a full-screen diff, close a comparison, or return to the graph."),
    ("Navigation", "universal", "contextActions", "O", "Context actions", "Show executable commands for the focused pane, including descriptions. Lowercase o also works where it is not the file opener."),
    ("Navigation", "universal", "optionMenu", "?", "Keyboard help", "Open this searchable reference. Type immediately; arrows select a command and Esc closes."),
    ("Navigation", "universal", "openRecentRepos", "<ctrl+r>", "Recent repositories…", "Choose another repository from the saved recent list."),
    ("Navigation", "universal", "quit", "q", "Quit", "Exit RustifiedGit. A running command must be cancelled first."),
    ("Search", "universal", "startSearch", "/", "Search focused pane", "Start a live list filter or search within diff/output text. Enter keeps a list filter; Esc cancels it."),
    ("Search", "universal", "fuzzySearch", "<ctrl+f>", "Fuzzy search", "Filter the commit graph, local/remote branches, or either file tab as you type. Commit search scans loaded history."),
    ("Search", "universal", "nextMatch", "n", "Next search match", "Find the next match in diff/output. Without a diff search, n selects the next file."),
    ("Search", "universal", "prevMatch", "N", "Previous search match", "Find the previous match in diff/output. Without a diff search, N selects the previous file."),
    ("Search", "universal", "filteringMenu", "<ctrl+s>", "Filters…", "Filter commit history by path, clear the path filter, or toggle changed-only and unchanged-file visibility."),
    ("Search syntax", "", "", "term1 term2", "Combine search terms", "Every space-separated term must match. Matching letters are highlighted."),
    ("Search syntax", "", "", "'term / !term", "Literal / exclude", "Prefix a term with an apostrophe for literal matching, or ! to exclude a substring."),
    ("Search syntax", "", "", "^term / term$", "Prefix / suffix", "Anchor a term to the beginning or end of the searched text; ^term$ is exact matching."),
    ("Search syntax", "", "", "Uppercase", "Smart case", "Lowercase searches ignore case. Any uppercase letter makes the query case-sensitive."),
    ("Working files", "universal", "select", "<space>", "Stage / unstage", "Toggle selected working files or folders in the index, including marked paths."),
    ("Working files", "files", "toggleStagedAll", "a", "Stage / unstage all", "Stage all working changes, or unstage when all changed files are staged."),
    ("Working files", "files", "commitChanges", "c", "Commit changes…", "Enter a commit message for staged changes."),
    ("Working files", "files", "commitChangesWithEditor", "C", "Commit in editor", "Open the configured Git editor for the commit message."),
    ("Working files", "files", "amendLastCommit", "A", "Amend last commit…", "Amend the most recent commit with staged changes and an edited message."),
    ("Working files", "files", "commitChangesWithoutHook", "w", "Commit without hooks…", "Enter a commit message and skip commit hooks for this commit."),
    ("Working files", "universal", "remove", "d", "Discard selected working changes…", "Confirm discarding changes only for the selected working paths."),
    ("Working files", "files", "viewResetOptions", "D", "Reset working tree…", "Unstage all, discard tracked changes, remove untracked files, or reset the workspace including submodules."),
    ("Working files", "files", "stashAllChanges", "s", "Save stash…", "Save working changes and untracked files in a named stash."),
    ("Working files", "files", "viewStashOptions", "S", "Stash options…", "Choose how to stash all changes, keep the index, or stash staged changes."),
    ("Working files", "files", "ignoreFile", "i", "Ignore selected path", "Add the selected path to .gitignore."),
    ("Working files", "files", "toggleIgnoredFiles", "I", "Show ignored files", "Toggle ignored files in the working-file list."),
    ("Working files", "files", "refreshFiles", "r", "Refresh files", "Reload working-file status without reloading unchanged commit history."),
    ("Working files", "files", "fetch", "f", "Fetch all remotes", "Fetch remote updates with progress and prune removed remote references."),
    ("File tree", "", "", "Enter", "Inspect full screen", "Open a file's diff full screen; on a folder, expand or collapse it."),
    ("File tree", "files", "toggleTreeView", "`", "Tree / flat view", "Switch between nested folders and a flat list of repository-relative paths."),
    ("File tree", "", "", "v", "Mark / unmark", "Toggle a working or commit path in the multiple-file selection. Folders include their descendants."),
    ("File tree", "", "", "Shift+↑ / Shift+↓", "Extend file selection", "Select a range from the starting row. Reverse direction to shrink the range. Folders include matching descendants."),
    ("File tree", "files", "deselectFile", "u", "Deselect file / folder", "Remove the hovered file or this folder's descendants from the selection, including a child of a selected folder."),
    ("File tree", "files", "clearSelection", "U", "Deselect all", "Clear every selected file while keeping focus in pane 7. Esc also clears the selection."),
    ("File tree", "files", "selectAll", "<ctrl+a>", "Select all matching files", "Select the files matching the active filter, including those inside collapsed folders. Checkboxes and the pane title show the selection."),
    ("File tree", "files", "toggleAllDirectories", "*", "Expand / collapse all", "Toggle all folders in the active file tree, including nested directories."),
    ("File tree", "files", "collapseAll", "-", "Collapse directories", "Collapse every folder in the active file tree."),
    ("File tree", "files", "expandAll", "=", "Expand directories", "Expand every folder in the active file tree."),
    ("File tree", "universal", "viewFileHistory", "F", "History of this path", "Show commits touching the selected file or folder; follow file renames."),
    ("File tree", "universal", "edit", "e", "Edit file", "Open the current working file in the configured editor."),
    ("File tree", "universal", "openFile", "o", "Open externally", "Open the current working path with the configured system opener."),
    ("File tree", "files", "copyFileInfoToClipboard", "y", "Copy path", "Copy the selected repository-relative path in working files or the HEAD tree."),
    ("File tree", "universal", "openDiffTool", "<ctrl+t>", "External diff tool", "Inspect the selected working, committed, compared or stashed file with Git's configured diff tool."),
    ("File tree", "files", "openMergeToolForFile", "<alt+m>", "External merge tool", "Open the configured Git merge tool for the selected conflicted working file."),
    ("File tree", "files", "openMergeOptions", "M", "Conflict actions…", "Choose ours/theirs, stage a resolution, or open the configured merge tool."),
    ("Commit files", "commitFiles", "cherryPickFiles", "V", "Cherry-pick selected files…", "Apply and stage only the selected commit's file changes without committing. Other paths remain untouched."),
    ("Commit files", "commitFiles", "checkoutCommitFile", "c", "Restore committed file…", "Restore full versions of the selected commit paths into the working tree."),
    ("Commit files", "universal", "remove", "d", "Discard selected working changes…", "Discard current uncommitted changes for the selected commit paths; this does not rewrite the commit."),
    ("Commit files", "", "", "v / Shift+↑ / Shift+↓", "Preview selected commit files", "Marked commit files share one combined diff. File actions apply only to selected paths."),
    ("HEAD tree", "universal", "nextTab", "]", "Changes / HEAD tree", "The Changes tab follows the selection. The HEAD tree tab always lists files committed at the current HEAD."),
    ("Commit graph", "", "", "Enter", "Inspect changed files", "Focus pane 7 for the hovered commit, or working files when Working tree / Index is selected."),
    ("Commit graph", "", "", "Shift+↑ / Shift+↓", "Fast commit scrolling", "Move by one visible page of commits. Older history loads automatically near the end."),
    ("Commit graph", "commits", "setHistoryLimit", "L", "Loaded commits…", "Choose and save the initial/page size, load more now, or retry a failed history request. Up to one million commits per page are supported."),
    ("Commit graph", "", "", ".", "Load older commits", "Load another page of history without moving the current selection."),
    ("Commit graph", "", "", "G", "Accelerate history", "Build Git's commit-graph metadata cache to speed up history queries in large repositories."),
    ("Commit graph", "commits", "checkoutCommit", "<space>", "Checkout commit…", "Confirm switching to the selected commit with a detached HEAD."),
    ("Commit graph", "commits", "viewResetOptions", "g", "Reset options…", "Choose soft, mixed or hard reset of the current branch to the selected commit."),
    ("Commit graph", "commits", "revertCommit", "t", "Revert commit…", "Create a new commit that reverses the selected commit's changes."),
    ("Commit graph", "commits", "cherryPickCopy", "C", "Copy commit", "Add the selected commit to the cherry-pick selection; copy multiple commits before switching branches."),
    ("Commit graph", "commits", "pasteCommits", "V", "Cherry-pick copied commits…", "Apply copied commits in their original source order on the current branch."),
    ("Commit graph", "commits", "createFixupCommit", "F", "Create fixup commit", "Commit staged changes as a fixup for the selected commit."),
    ("Commit graph", "commits", "createTag", "T", "Create tag…", "Create a named tag pointing to the selected commit."),
    ("Commit graph", "commits", "startInteractiveRebase", "i", "Interactive rebase", "Open the interactive rebase editor starting at the selected commit."),
    ("Commit graph", "commits", "renameCommit", "r", "Reword / amend…", "Change the selected commit's message; older commits use rebase."),
    ("Commit graph", "commits", "renameCommitWithEditor", "R", "Reword / rebase…", "Amend the HEAD message or open the rebase editor for an older commit."),
    ("Commit graph", "commits", "copyCommitAttributeToClipboard", "y", "Copy hash", "Copy the full selected commit ID to the clipboard."),
    ("Commit graph", "commits", "viewBisectOptions", "b", "Bisect options…", "Start a bisect, mark good/bad, skip a commit, or reset bisect."),
    ("Commit graph", "universal", "nextTab", "]", "Commits / reflog", "Toggle the graph between commit history and the reflog."),
    ("Branches", "", "", "<space>", "Toggle branch in graph", "Enable or hide a local or remote branch without checking it out. A group toggles all descendant branches; visibility is saved per repository."),
    ("Branches", "", "", "Enter", "Checkout reference / toggle group", "Switch to the selected local branch, track a remote branch, or detach at a tag. On a group, expand or collapse it."),
    ("Branches", "universal", "new", "n", "Create branch…", "Create a branch from the selected reference or from HEAD when a group is selected."),
    ("Branches", "branches", "checkoutBranchByName", "c", "Checkout by name…", "Enter the name of the branch to check out."),
    ("Branches", "branches", "checkoutPreviousBranch", "-", "Previous branch", "Switch to the previously checked-out branch."),
    ("Branches", "branches", "renameBranch", "R", "Rename branch…", "Rename the selected local branch."),
    ("Branches", "branches", "mergeIntoCurrentBranch", "M", "Merge reference…", "Merge the selected reference into the current branch."),
    ("Branches", "branches", "rebaseBranch", "r", "Rebase onto reference…", "Rebase the current branch onto the selected reference."),
    ("Branches", "branches", "forceCheckoutBranch", "F", "Force checkout…", "Confirm switching to the selected branch and discarding tracked working changes."),
    ("Branches", "branches", "createTag", "T", "Tag reference…", "Create a tag at the selected reference's commit."),
    ("Branches", "universal", "remove", "d", "Delete reference…", "Confirm deleting the selected local branch, remote-tracking branch or tag."),
    ("Branches", "", "", "u", "Set upstream", "Set the current branch's upstream to the selected branch."),
    ("Branches", "", "", "y", "Copy ref name", "Copy the full selected branch or tag name."),
    ("Branches", "", "", "f", "Fetch all remotes", "Fetch and prune remote references with live progress."),
    ("Branches", "universal", "nextTab", "]", "Branches / tags / worktrees", "Cycle the tabs in the local branches pane."),
    ("Worktrees", "universal", "newWorktree", "w", "Worktree from reference…", "Create a worktree at a chosen path from the selected branch or tag."),
    ("Worktrees", "", "", "n", "New worktree…", "In the Worktrees tab, enter the path for a new worktree."),
    ("Worktrees", "", "", "Enter", "Open worktree", "Switch RustifiedGit to the selected worktree."),
    ("Worktrees", "", "", "d", "Remove worktree…", "Confirm removing the selected worktree."),
    ("Stashes", "", "", "4 → 7", "Inspect stashed files", "Selecting a stash populates pane 7 with its changes against its original base, including untracked files. Select a file to preview its diff in pane 0."),
    ("Stashes", "", "", "Enter", "Inspect whole stash", "Show the selected stash's full patch. Enter on a file in pane 7 opens only that file full screen."),
    ("Stashes", "", "", "<space>", "Apply stash…", "Apply the selected stash while keeping it in the stash list."),
    ("Stashes", "stash", "popStash", "g", "Pop stash…", "Apply the selected stash and remove it from the list on success."),
    ("Stashes", "", "", "d", "Drop stash…", "Confirm deleting the selected stash."),
    ("Stashes", "", "", "s", "Save stash…", "Save working changes and untracked files in a named stash."),
    ("Compare", "universal", "compareRevisions", "B", "Compare revisions…", "Compare two commits, branches, tags or typed revision names. A → B shows changes needed to reach B."),
    ("Compare", "", "", "B → Mark", "Mark comparison start (A)", "Mark a commit or branch, select another revision, then choose Compare marked A → B."),
    ("Compare", "", "", "B → HEAD", "Compare HEAD with selection", "Inspect differences from the current HEAD to the selected revision."),
    ("Compare", "", "", "B → base", "Compare with base branch", "Enter the base branch; compare its common ancestor with the selected revision."),
    ("Compare", "", "", "B → names", "Compare two revision names", "Enter both endpoints explicitly, such as main and feature/login."),
    ("Compare", "", "", "B → swap", "Swap comparison direction", "Reverse the comparison endpoints; file statuses and patches reverse too."),
    ("Compare", "", "", "B → close", "Close comparison / clear mark", "Return to the graph or clear the saved comparison start. Comparisons are read only."),
    ("Diff", "", "", "↑ / ↓", "Scroll within hunk", "Scroll line by line through a large hunk; PageUp/PageDown scroll a screen."),
    ("Diff", "", "", "← / →", "Previous / next hunk", "Jump between diff hunks without changing the selected file."),
    ("Diff", "main", "toggleSelectHunk", "a", "Hunk / line selection", "Choose whether staging applies to an entire hunk or only the selected changed line."),
    ("Diff", "", "", "<space>", "Stage / unstage selection", "Stage or unstage the selected working-tree hunk or line."),
    ("Diff", "", "", "d", "Discard selection…", "Confirm discarding the selected unstaged working-tree hunk or line."),
    ("Diff", "universal", "increaseContextInDiffView", "}", "More context", "Increase the unchanged context shown around each diff hunk."),
    ("Diff", "universal", "decreaseContextInDiffView", "{", "Less context", "Decrease the unchanged context shown around each diff hunk."),
    ("Diff", "universal", "toggleWhitespaceInDiffView", "<ctrl+w>", "Toggle whitespace filter", "Show or ignore whitespace-only differences. Turn this off before applying a patch."),
    ("Diff", "", "", "H / L", "Scroll left / right", "Reveal earlier or later columns in long diff lines."),
    ("Diff", "", "", "n / N", "Next / previous file", "Inspect adjacent files; when a diff search is active, these keys navigate search matches instead."),
    ("Diff", "universal", "toggleDiffPreview", "~", "Automatic preview", "Save whether hovering a file previews its diff automatically. Enter or 0 explicitly opens the diff regardless of preview limits."),
    ("Details / Output", "universal", "extrasMenu", "@", "Details / output…", "Choose the Details or Output tab in pane 6."),
    ("Details / Output", "", "", "Enter", "Inspect files from details", "Focus pane 7 from the commit-details tab."),
    ("Details / Output", "", "", "End", "Follow output", "Jump to the latest command output and follow new lines. Arrow keys scroll without following."),
    ("Details / Output", "", "", "—", "Live command progress", "Output shows Git's progress, transfers and command steps. A running command blocks additional commands with a popup."),
    ("Operations", "universal", "refresh", "R", "Refresh", "Refresh working files and references. In branches and graph, R has its pane-specific rename/rebase meaning."),
    ("Operations", "universal", "pull", "p", "Pull options…", "Choose fast-forward only, merge, rebase, autostash, confirmed overwrite, or fetch only."),
    ("Pull", "", "", "p → 1", "Fast-forward only", "Update only when the upstream can be fast-forwarded; stop if histories diverged."),
    ("Pull", "", "", "p → 2", "Merge remote changes", "Allow a merge commit when the local and upstream histories diverge."),
    ("Pull", "", "", "p → 3", "Rebase local commits", "Replay local commits on top of the upstream history."),
    ("Pull", "", "", "p → 4", "Rebase + autostash", "Temporarily stash tracked working changes, rebase, then restore them."),
    ("Pull", "", "", "p → 5", "Overwrite from upstream…", "Fetch and confirm replacing the local branch and tracked working changes with the upstream state."),
    ("Pull", "", "", "p → 6", "Fetch only", "Download upstream updates without merging them into the current branch."),
    ("Operations", "universal", "push", "P", "Push options…", "Choose a normal push, dry run, force with lease, force push, or set the upstream."),
    ("Push", "", "", "P → 1", "Push normally", "Push the current branch or selected local reference using normal Git checks."),
    ("Push", "", "", "P → 2", "Force push with lease…", "Replace remote history only if the remote tip matches the known remote reference."),
    ("Push", "", "", "P → 3", "Force push…", "Confirm overwriting remote history, including commits made by others."),
    ("Push", "", "", "P → 4", "Dry run push", "Show what a push would do without changing the remote."),
    ("Push", "", "", "P → upstream", "Set upstream and push", "Publish the current branch to origin and configure its upstream."),
    ("Operations", "universal", "createRebaseOptionsMenu", "m", "Operation controls…", "Continue or abort a merge, rebase, cherry-pick or revert; skip a rebase commit."),
    ("Operations", "universal", "executeShellCommand", ":", "Run command…", "Run a Git or shell command with captured live output in pane 6."),
    ("Operations", "", "", "!", "Interactive command…", "Restore the terminal temporarily to run an interactive command or editor."),
    ("Operations", "", "", "<ctrl+c>", "Cancel command", "Cancel the active command and its child processes, including from a busy-command popup. When idle, Ctrl+c quits."),
    ("Repositories", "", "", "r", "Repository operations…", "In pane 1, fetch, choose pull/push options, initialize/update submodules, or open an interactive command."),
    ("Submodules", "universal", "nextTab", "]", "Repositories / Submodules", "In pane 1, switch between recent repositories and all submodules declared in .gitmodules, including clean and uninitialized submodules. [ switches back."),
    ("Submodules", "", "", "Enter", "Open / initialize submodule", "Open an initialized submodule as its own repository. An uninitialized entry first initializes recursively; press Enter again when it completes. Nested submodules have their own tab."),
    ("Submodules", "", "", "i", "Initialize/update submodule", "Initialize or update the selected submodule and its nested submodules to the recorded commits, with live progress in pane 6."),
    ("Submodules", "", "", "a", "Initialize/update all submodules", "Initialize or update every submodule recursively to its recorded commit."),
    ("Submodules", "", "", "y", "Copy submodule path", "Copy the selected submodule's full local path."),
    ("Submodules", "", "", "Enter on parent", "Return to parent repository", "The Submodules tab contains a Parent repository row when browsing inside a submodule."),
];
