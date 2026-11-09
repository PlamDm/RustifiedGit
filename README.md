# PD LazyGit — Rust

A native Rust terminal Git client, ported from the `pd_development` branch of
[PlamDm/pd_lazygit](https://github.com/PlamDm/pd_lazygit), revision
`d9cd093e7af6fa0c79c22b62a9113928c8d00d16`.

The source is independent of the Go checkout. The executable needs Git on PATH;
it does not call the Go application. [PORTING.md](PORTING.md) records exactly
which workflows have native implementations and which compatibility work remains.

## Run

```sh
cd ~/Playground/pd_lazygit_rust
cargo build --release --locked
./target/release/pd-lazygit -p ~/Playground/pd_lazygit
```

The repository pins Rust 1.88.0 through `rust-toolchain.toml`. Rustup selects it
for this project without changing your default toolchain. The optimized release
profile enables thin LTO and a single code-generation unit.

To open the Rust project itself:

```sh
./target/release/pd-lazygit -p .
```

The command is `pd-lazygit`, so your installed Go `lazygit` stays available.

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

Press `/` or `Ctrl+f` in the graph or either file tab for live **fzf-style search**.
Type to filter, use arrows or Ctrl+n/p to select, Enter to keep the filter,
and Esc to cancel and restore the previous selection. Esc after accepting
clears the filter. Commit matches are ranked, prioritizing subjects; file
matches retain their directory hierarchy and reveal collapsed descendants.
Terms separated by spaces must all match; `'term` requires a literal substring,
`!term` excludes one, and `^` / `$` anchor terms. Lowercase queries ignore case;
uppercase queries use smart case. This is built in and needs no fzf executable.

Local branches also provide tags and worktrees through `[`/`]`. Graph history
loads the first 1,000 commits independently of file status, then automatically
loads another 1,000 when you approach the end. `.` explicitly loads another page.
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

## Your fork's workflows

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

Standard workflows include whole-file, directory, hunk and individual-line
staging/unstaging, discarding, commit/amend/editor, branches, tags, remotes,
merge/rebase controls, cherry-pick/revert/fixup, stashes, worktrees, conflict
tools, submodule actions, history/reflog, fuzzy file filtering, and recent repos.
Press `?` for keys, `:` for a captured command, or `!` for an interactive command.
Use `!` for Git operations that need a terminal credential prompt; configured
credential helpers and SSH agents continue to work through Git.

## Configuration

Rust preferences default to `~/.config/pd-lazygit-rust` (`%APPDATA%` on Windows,
or `$XDG_CONFIG_HOME` when set). Nothing is written to the Go configuration
unless you explicitly select its directory.

Existing LazyGit YAML can be selected with:

```sh
./target/release/pd-lazygit --use-config-dir ~/Playground/config/lazygit
./target/release/pd-lazygit --use-config-file ./config.yml,./local.yml
./target/release/pd-lazygit --validate-config --use-config-file ./config.yml
```

Settings are merged in order. This version supports the fork's file/preview
settings, refresh settings, common action key remaps, editor/open commands,
and custom commands with selected-value templates and input prompts. It does
not interpret every upstream LazyGit YAML setting; see the compatibility table.

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

`rust-state.json` stores toggles, per-repository visible refs and recent repositories. Existing `config.yml`
and Go `state.yml` files are never overwritten. Destructive Git actions require
confirmation in the UI. Network and write operations only run after an action
or an enabled scheduled fetch; repository reads set `GIT_OPTIONAL_LOCKS=0`.

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
./target/release/pd-lazygit --snapshot -p /path/to/repo
./target/release/pd-lazygit --render 120x40 -p /path/to/repo
./target/release/pd-lazygit --benchmark --iterations 20 -p /path/to/repo
python3 scripts/compare_startup.py --go ./target/benchmark-go-lazygit
```

The startup comparison requires an optimized build of the original fork; don't
use a Go build with `-gcflags='all=-N -l'` as a performance baseline. Repository
size, filesystem cache, Git configuration, hooks and authentication affect
results. See [PERFORMANCE.md](PERFORMANCE.md) for recorded measurements.

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
- `config.rs`: YAML compatibility and separate persisted Rust state.

The port retains the original MIT notice in [LICENSE](LICENSE). Technical
references: [Ratatui 0.29](https://docs.rs/ratatui/0.29.0/ratatui/),
[Git porcelain status](https://git-scm.com/docs/git-status),
[Git patch format](https://git-scm.com/docs/git-diff).
