# Recorded performance

Measured locally on macOS arm64 on November 9, 2026, with Rust 1.88.0,
Go 1.27.3 and Git 2.56.0. Raw samples are in
[benchmarks/macos-arm64.json](benchmarks/macos-arm64.json).

## Populated terminal startup

The benchmark creates a disposable repository with tracked files, disables
automatic fetch/refresh and startup popups in both clients, then measures
process launch to a readiness marker appearing in the Files panel. These samples
were recorded before the numbered graph layout was introduced. Both
clients use a real 140×40 PTY. Runs alternate Go and Rust.

| Tracked files | Runs per client | Optimized Go median | Rust release median | Go / Rust ratio |
| ---: | ---: | ---: | ---: | ---: |
| 5,003 | 5 | 78.31 ms | 33.94 ms | 2.31× |
| 20,003 | 3 | 167.50 ms | 62.31 ms | 2.69× |

The 5,003-file measurement uses the original port's release build. The larger-fixture
measurement was collected on an earlier release candidate with the same
repository-worker/tree/rendering architecture. Its first Rust sample was
**295.39 ms**, versus **208.13 ms** for Go; subsequent Rust samples were
62.31 and 60.35 ms. First launches and cold filesystem caches can therefore
behave differently from the medians. These measurements do not establish
a universal cold-start speedup.

The current comparison script modifies the marker file so it is visible in the
new layout's default changed-files pane. Its output identifies the dirty fixture
and each application's default layout; fresh samples should be kept separate
from the earlier clean-tree measurements above.

The Go reference is `pd_development` at
`d9cd093e7af6fa0c79c22b62a9113928c8d00d16`, built with ordinary optimization:

```sh
cd ../pd_lazygit
go build -mod=vendor -trimpath -ldflags=-s \
  -o ../pd_lazygit_rust/target/benchmark-go-lazygit .
cd ../pd_lazygit_rust
cargo build --release --locked
python3 scripts/compare_startup.py --go ./target/benchmark-go-lazygit
```

Do not compare against the existing Go debug build made with
`-gcflags='all=-N -l'`. The Rust program also does not yet implement every
upstream feature; see [PORTING.md](PORTING.md). Startup comparisons describe
the shared repository-browsing workload, not full application equivalence.

## Repository refresh

On the actual Go fork checkout (2,572 files, 5 references, 300 loaded commits),
20 read-only Rust snapshots measured:

| Statistic | Time |
| --- | ---: |
| First snapshot | 32.69 ms |
| Median | 19.18 ms |
| 95th percentile | 23.40 ms |
| Minimum | 17.42 ms |

This is an internal Rust backend measurement; no equivalent Go backend result
was measured. It is not a claim that all Git operations became faster.

```sh
./target/release/pd-lazygit --benchmark --iterations 20 -p ../pd_lazygit
```

## Responsiveness guarantees in the design

- Snapshot, preview and write jobs have separate workers. Running a slow
  command does not block navigation or rendering.
- Preview requests coalesce; a new selection cancels stale Git processes.
- Working-file/index/object size checks use metadata. Automatic diff reads
  consume at most the configured output limit plus one byte before stopping.
- The tracked-file list is reused while the index stamp is unchanged. A
  background refresh with unchanged file entries reuses the tree rows.
- File lists style only visible rows; diffs render only visible lines.
- Idle frames are not redrawn. Command output is streamed through a bounded
  channel and a bounded UI history.
- Git is the execution engine for mutations, hooks, signing, credential helpers
  and remotes. Network, filesystem and external-tool costs remain their own.

Original port verification: 40 Rust tests, warning-free Clippy,
format checks, an optimized build, and a real-terminal smoke run covering
navigation, staging, commit, saved settings, live output and cancellation.
