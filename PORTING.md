# Functionality and compatibility

Reference: `PlamDm/pd_lazygit`, branch `pd_development`, commit
`d9cd093e7af6fa0c79c22b62a9113928c8d00d16`.

This is a new Rust implementation, not a line-by-line mechanical translation.
The Go project contains the full upstream LazyGit application as well as your
changes. A working native implementation does not establish complete upstream
feature parity. This file keeps that boundary explicit.

## Fork behavior

| Behavior | Rust implementation | Verification |
| --- | --- | --- |
| Clean tracked files and dimmed directories | Native repository tree | Repository and tree tests |
| Tree/flat/collapse/search/changed-only | Native | Tree and UI tests |
| Persistent ignored-file visibility | Native | Configuration/UI tests |
| Ignored files excluded from stage/discard | Native, including directory selections | Real Git tests |
| Honor `status.showUntrackedFiles=no` | Native | Real Git test |
| File/directory history, follow file renames | Native | Real Git history test |
| History from Commit Files | Native action | Shared history dispatch |
| Per-file Git difftool/mergetool | Native subprocess handoff | Git configuration delegated unchanged |
| Persist automatic-preview preference, including off | Native | Configuration/UI tests and PTY smoke |
| Check worktree, index/conflict stages, committed versions | Native Git metadata queries | Real Git size-limit tests |
| Stop oversized diff output; explicit preview bypass | Native bounded readers | Real Git preview tests |
| Show output while a command runs | Native independent command worker | Streaming and PTY tests |
| Panel 6 focuses command output | Native | UI test |
| Submodule reset/clean stays off UI thread | Native cancellable command jobs | Shared process-group cancellation |
| Working-tree nuke with submodules | Stash dirty/untracked submodule content, reset submodules, reset HEAD, clean untracked files; keep ignored files | Real Git submodule test |

## Core Git workflows

| Area | Implemented |
| --- | --- |
| Files/index | File/directory/range selection, stage/unstage, discarded changes, ignores, editor/opener |
| Partial changes | Hunk or individual added/removed line staging and unstaging; confirmed discard |
| Commits | Message/editor, amend, hooks and signing via Git, fixup, revert, cherry-pick copied commits, reset options |
| History | Paginated commits, reflog, path filter with rename following, commit file list and diffs |
| Branches | Create, checkout/force checkout/previous, rename/delete, merge/rebase, set upstream |
| Remotes/tags | List remote refs, fetch/pull/push, local tag create/delete, tag push |
| Worktrees | List, create, open and remove |
| Stash | Save tracked/untracked/staged/keep-index, preview, apply, pop and drop |
| Conflict/operation control | External merge tool, ours/theirs, stage resolutions, rebase/merge/cherry-pick/revert continue/abort; bisect controls |
| Advanced interactive Git | `!` restores the terminal and runs Git/editor interactively, then refreshes |
| UI | Six panels, keyboard/mouse, fuzzy list filters, resize, fullscreen, horizontal diff scroll, bounded output history |

## Remaining upstream compatibility work

The following do not have equivalent dedicated Rust UI implementations yet:

- Upstream's full configurable side-panel/tab layouts, themes, icons and
  translations. The Rust UI currently has one layout and English labels.
- The full Go-template custom-command language, dynamic menu suggestions,
  template functions that execute Git, and complex prompt/menu forms. Selected
  file/branch/tag/commit/stash values, indexed input responses, `.Form` input
  keys, and `quote` are supported; unsupported expressions return an error.
- Inline conflict-marker editing, patch building across multiple arbitrary
  selections, and the complete interactive rebase todo-list editor with all
  of upstream's context-specific keybindings. External Git/editor workflows
  are available for these operations.
- Hosting-provider PR/review integrations, Git-flow menus, full reflog undo/redo
  UI and automatic update downloads.
- Exhaustive compatibility with every upstream YAML option and platform. Git
  operations are portable, but the recorded runtime validation is on macOS.
  The non-UTF-8 filesystem test additionally runs on Unix filesystems that
  allow those names; macOS validates native path bytes without creating an
  invalid filename.

These are migration tasks, not completed features. The original Go checkout
is the behavioral reference and remains available during that work. There is
no hidden Go fallback inside the Rust executable.
