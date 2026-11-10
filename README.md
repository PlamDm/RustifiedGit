# RustifiedGit

A keyboard-driven terminal Git client written in Rust. It provides a commit
graph, repository and branch navigation, file and diff inspection, and common
Git operations in one interface. Git must be available on `PATH`.

## Run

```sh
cd ~/Playground/rustifiedgit
cargo build --release --locked
./target/release/rustifiedgit -p /path/to/repository
```

The repository pins Rust 1.88.0 through `rust-toolchain.toml`. Rustup selects it
for this project without changing your default toolchain. The optimized release
profile enables thin LTO and a single code-generation unit.

To open the Rust project itself:

```sh
./target/release/rustifiedgit -p .
```

The executable is named `rustifiedgit`.

## Numbered panes

The keyboard layout places repository navigation on the left, the commit graph
in the middle, and commit details and changed files on the right:

```text
┌ 1 Repositories ┐┌ 5 Commit graph ─────────┐┌ 6 Commit details ┐
├ 2 Local ──────┤│ Working tree / Index    ││ Message and body │
│ [x] main      ││ * Merge feature        ││ Author, date, SHA│
│ [ ] feature   ││ |\                     │├ 7 Changed files ┤
├ 3 Remote ─────┤│ | * Feature commit     ││ M src/app.rs     │
│ [ ] origin/…  ││ * | Main commit        ││ A src/ui.rs      │
├ 4 Stashes ────┤└────────────────────────┴┴──────────────────┤
│ stash@{0}     │┌ 0 Diff ───────────────────────────────────┤
└───────────────┘└───────────────────────────────────────────┘
                  8 Command output (drawer)
```

Press a number to focus its pane, `Tab`/`Shift+Tab` to cycle, and `+` to zoom.
In local or remote branches, **Space toggles graph visibility**; **Enter checks
out** the branch. The graph combines enabled refs and their shared ancestors
without duplicate commits, preserving Git's merge connections. Visibility is
saved per repository. Initially the graph shows the current branch.

Graph rows reserve the right edge for the committer's initials and a date such
as `PD 26 Nov 2026`. Long subjects end with an ellipsis before those columns;
the details pane keeps the full message. Initials and dates use the committer,
which may differ from the original author when changes are cherry-picked.

Local and remote branches form nested groups separated by `/`: a branch named
`feature/user/login` appears under `feature/` → `user/` → `login`. Remotes add a
top-level group such as `origin/`. `Enter` expands/collapses a group;
Left/Right navigates its hierarchy. Space enables all branches in a group, or
disables them when all are enabled. On a branch leaf, Enter still checks out
that branch. Searching reveals matching branches inside collapsed groups.
Set `gui.branchGroupDelimiter` to another separator, or an empty string for a flat list.

`Shift+Up`/`Shift+Down` in the graph moves by approximately one visible page of
commits; the step adapts to pane height and zoom. Plain arrows move one row.
PageUp/PageDown also adapt to the focused pane's height. Shift+arrows in the
working-file pane keep their range-selection behavior.

`O` opens the focused pane's executable actions, with shortcuts and a description
of the selected action. `o` also opens actions where it has no file-opening
role. In working files, committed files, diff views and the graph's working-tree
row, `o` keeps its external file opener behavior. Use Up/Down to choose an action,
Enter or its shortcut to run it, and Esc to close. The bottom hints come from
the same action list. Menus honor key remaps, disabled bindings and custom
commands; repository-changing actions retain their existing confirmations.

Move with `j`/`k` in pane 5 to update the full commit message, metadata, changed
files and selected file's diff. Focus stays in the graph. `Enter` moves to files;
another `Enter` opens the file's diff full screen. `Esc` restores the pane layout
and returns to the same file. Pane 6 scrolls wrapped messages and metadata.
In a diff, Up/Down or `j`/`k` scroll through a selected hunk immediately;
PageUp/PageDown scroll by a screen. Left/Right jump between hunks. Press `a`
to switch to individual-line selection when you want to stage specific lines.
**Working tree / Index** appears directly above the active branch's tip, even
when another enabled branch has newer commits. The active branch is bold in
the branch list and graph. If HEAD is outside the loaded or filtered history,
the working-tree row stays at the top so its actions remain available.

Pane 7 has **Changes** and **HEAD tree** tabs; `[` / `]` switches between them.
Changes follows the hovered commit, or shows uncommitted changes when the
working-tree row is selected. HEAD tree shows every tracked path committed at
current HEAD, independent of the hovered commit and dirty working files. Enter
opens that file's committed contents full screen. Each tab keeps its own search
and selection. `Ctrl+s` can still adjust working-file visibility.

The header shows **AHEAD n · BEHIND n** relative to the active branch's configured
upstream; branch rows show compact `↑n ↓n` counts. Counts use the locally known
remote refs and update on refresh/fetch. Missing or unset upstreams are labeled
instead of being reported as zero.

Press `/` or `Ctrl+f` in the graph, branch panes or either file tab for live **fzf-style search**.
Type to filter, use arrows or Ctrl+n/p to select, Enter to keep the filter,
and Esc to cancel and restore the previous selection. Esc after accepting
clears the filter. Commit matches are ranked, prioritizing subjects; file
matches retain their directory hierarchy and reveal collapsed descendants.
Matched letters are yellow, bold and underlined in graph rows and file paths,
including after you accept the filter.
Terms separated by spaces must all match; `'term` requires a literal substring,
`!term` excludes one, and `^` / `$` anchor terms. Lowercase queries ignore case;
uppercase queries use smart case. This is built in and needs no fzf executable.

Local branches also provide tags and worktrees through `[`/`]`. Graph history
loads the first 1,000 commits independently of file status by default. In the
graph, open `o` → **Loaded commits…** (or press `L`) to choose an initial/page
size from presets or enter a custom positive number. The menu shows the actual
loaded count and current requested target. Your choice persists across restarts;
`git.commitLimit` in YAML sets the default when no saved choice exists. Values
from 1 through 1,000,000 are supported, and the `o` history menu can change the
initial page size while automatic paging continues. History reads have a
separate 512 MiB output budget; ordinary Git commands retain their smaller
limit. A failed history read retains already loaded commits and pauses automatic
retries. Press `L` to retry or choose a smaller page size. Long error messages
can be scrolled inside the popup.
Older commits still load automatically near the end, one chosen page at a time;
`.` or **Load more now** explicitly requests another page. This sets the initial
amount and page size, rather than a fixed cap on all history loading.
The count's `+` means older commits remain. History is reused while refs and
filters stay unchanged. A search covers loaded commits (subject, hash, author, committer and ref labels),
then ranks matches and hides edges between omitted rows. It does not scan the
full repository history or file contents. Reflog remains available through `[`/`]` in the graph.

For a large fresh clone, press `G` in the graph to build Git's commit-graph
acceleration cache. The first build can take several seconds; subsequent
topological history queries can be much faster. This writes Git metadata without
changing commits, branches, the index or working files. The equivalent command is:

```sh
git -C /path/to/repo commit-graph write --reachable
```

## Repository workflows

- The full repository file view includes unchanged tracked files, dimmed with their clean directories.
  `I` includes ignored files. Staging and discarding exclude ignored files.
- `F` opens file or directory history. File history follows renames. Use `Ctrl+s`
  to clear the history filter. These actions work from Commit Files too.
- `Ctrl+t` runs Git's configured diff tool. `Alt+m` runs its configured merge
  tool for a selected file. Git's Beyond Compare or other tool configuration
  remains in effect.
- `~` saves the automatic preview preference. Automatic file previews inspect
  file/index/object sizes before loading content: 2 MiB per file version and
  512 KiB of diff output by default. Explicit `0`/Enter bypasses those limits.
- `8` focuses live command output. Long operations run in the background; their
  output stays visible. `Ctrl+c` cancels their process group, including child
  Git/SSH/hook processes. Read workers cancel stale selections separately.
  Invoking another operation opens a popup naming the active and blocked commands;
  the new command is not queued. Enter/Esc dismisses it; Ctrl+c cancels the active
  command even while the popup is open. The log also records each command step.

`p` opens pull options: fast-forward only, merge, rebase, rebase with autostash,
fetch only, or overwrite the local branch with its fetched upstream. Overwrite
requires confirmation and discards local-only commits and tracked changes; it
stashes submodule changes and keeps unrelated untracked files. An unavailable
upstream or failed fetch stops the operation before reset.

`P` opens push options: normal, force with lease, force, dry run, or set upstream
and push to origin. Force modes require confirmation. Current-branch pushes use
Git's configured destination; selecting another local branch or tag in the branch
pane targets that reference at origin, named in the menu title. Repository actions
and context actions open the same option menus.

Standard workflows include whole-file, directory, hunk and individual-line
staging/unstaging, discarding, commit/amend/editor, branches, tags, remotes,
merge/rebase controls, cherry-pick/revert/fixup, stashes, worktrees, conflict
tools, submodule actions, history/reflog, fuzzy file filtering, and recent repos.
Press `?` for keys, `:` for a captured command, or `!` for an interactive command.
Use `!` for Git operations that need a terminal credential prompt; configured
credential helpers and SSH agents continue to work through Git.

## Configuration

RustifiedGit preferences default to `~/.config/rustifiedgit` (`%APPDATA%` on
Windows, or `$XDG_CONFIG_HOME` when set). Preferences from an earlier
RustifiedGit config directory are reused automatically until the new directory
exists.

An alternate YAML configuration directory or file can be selected with:

```sh
./target/release/rustifiedgit --use-config-dir ~/.config/rustifiedgit
./target/release/rustifiedgit --use-config-file ./config.yml,./local.yml
./target/release/rustifiedgit --validate-config --use-config-file ./config.yml
```

Settings are merged in order. Configuration supports file and preview options,
refresh settings, action key remaps, editor/open commands, and custom commands
with selected-value templates and input prompts.

```yaml
gui:
  showUnchangedFiles: true
  showFileTree: true
  branchGroupDelimiter: '/'
  diffPreviewMaxFileBytes: 2097152
  diffPreviewMaxBytes: 524288
  sidePanelWidth: 0.32
  mouseEvents: true
  commandLogSize: 1000
git:
  autoRefresh: true
  autoFetch: true
refresher:
  refreshInterval: 3
  fetchInterval: 600
keybinding:
  files:
    toggleIgnoredFiles: I
  universal:
    toggleDiffPreview: '~'
```

`rust-state.json` stores toggles, per-repository visible refs and recent
repositories. Destructive Git actions require confirmation in the UI. Network
and write operations only run after an action or an enabled scheduled fetch;
repository reads set `GIT_OPTIONAL_LOCKS=0`.

## Development and verification

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo build --release --locked
python3 scripts/terminal_smoke.py
```

Equivalent `just build`, `just test`, `just lint`, `just check` and `just run`
recipes are included. Tests create disposable repositories and exercise real
Git operations; they do not stage, commit, or reset your development repository.
The PTY smoke script runs on macOS/Linux and tests the actual terminal session.

For read-only inspection and reproducible performance measurements:

```sh
./target/release/rustifiedgit --snapshot -p /path/to/repo
./target/release/rustifiedgit --render 120x40 -p /path/to/repo
./target/release/rustifiedgit --benchmark --iterations 20 -p /path/to/repo
```

Repository size, filesystem cache, Git configuration, hooks and authentication
affect performance measurements.

## Architecture

- `git.rs`: NUL-delimited Git protocols, native paths, index cache, object-size
  checks and typed operations. All mutation semantics remain Git's own.
- `process.rs`: bounded pipe reads, streaming output, cancellation and child
  process-group cleanup.
- `worker.rs`: independent history/status/preview reads and serialized write jobs.
- `tree.rs`: directory aggregation, clean/ignored styling metadata and filters.
- `branches.rs`: recursive branch groups and collapsed/searchable rows.
- `actions.rs`: available context commands shared by action menus and bottom hints.
- `patch.rs`: complete hunk and selected-line patches applied by `git apply`.
- `app.rs`: state transitions, keybindings, dialogs and command dispatch.
- `ui.rs`: Ratatui rendering; only visible file rows and diff lines are styled.
- `config.rs`: YAML configuration, preferences and saved application state.

RustifiedGit is licensed under MIT; see [LICENSE](LICENSE). Technical
references: [Ratatui 0.29](https://docs.rs/ratatui/0.29.0/ratatui/),
[Git porcelain status](https://git-scm.com/docs/git-status),
[Git patch format](https://git-scm.com/docs/git-diff).
