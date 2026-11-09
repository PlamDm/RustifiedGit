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

## Your fork's workflows

- Files includes unchanged tracked files, dimmed with their clean directories.
  `I` includes ignored files. Staging and discarding exclude ignored files.
- `F` opens file or directory history. File history follows renames. Use `Ctrl+s`
  to clear the history filter. These actions work from Commit Files too.
- `Ctrl+t` runs Git's configured diff tool. `Alt+m` runs its configured merge
  tool for a selected file. Git's Beyond Compare or other tool configuration
  remains in effect.
- `~` saves the automatic preview preference. Automatic file previews inspect
  file/index/object sizes before loading content: 2 MiB per file version and
  512 KiB of diff output by default. Explicit `0`/Enter bypasses those limits.
- `6` focuses live command output. Long operations run in the background; their
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

`rust-state.json` stores toggles and recent repositories. Existing `config.yml`
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
- `worker.rs`: coalesced read requests and independent serialized write jobs.
- `tree.rs`: directory aggregation, clean/ignored styling metadata and filters.
- `patch.rs`: complete hunk and selected-line patches applied by `git apply`.
- `app.rs`: state transitions, keybindings, dialogs and command dispatch.
- `ui.rs`: Ratatui rendering; only visible file rows and diff lines are styled.
- `config.rs`: YAML compatibility and separate persisted Rust state.

The port retains the original MIT notice in [LICENSE](LICENSE). Technical
references: [Ratatui 0.29](https://docs.rs/ratatui/0.29.0/ratatui/),
[Git porcelain status](https://git-scm.com/docs/git-status),
[Git patch format](https://git-scm.com/docs/git-diff).
