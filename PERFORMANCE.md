# Recorded performance

Measured locally on macOS arm64 on November 9, 2026, with Rust 1.88.0,
Go 1.27.3 and Git 2.56.0. Raw samples are in
[benchmarks/macos-arm64.json](benchmarks/macos-arm64.json).

## History loading in the LLVM clone

The full LLVM clone exposed a history-loading bottleneck. Before Git's
commit-graph acceleration index existed, the graph query spent about 3.5 seconds
walking history even when asked for only 300 commits. The same metadata query
without `--graph --topo-order` took about 18 ms. File status separately took
about 610 ms, and the original snapshot loader waited for both before publishing
commits.

Building `git commit-graph write --reachable` once took 5.91 seconds and wrote
a 37.3 MB metadata cache. It did not change commits, refs, the index or working
files. `G` in the graph runs this operation for other large clones.

| Git history query | Median before index | Median with index |
| --- | ---: | ---: |
| 300 commits with graph | 3,519 ms | 46 ms |
| 1,000 commits with graph | — | 44 ms |
| 5,000 commits with graph | 3,532 ms | 106 ms |

The new history worker publishes commits independently of working-file status,
reuses its shared commit list while refs/filters are unchanged, starts with
1,000 commits, and fetches another 1,000 near the end. Each extension currently
requeries the larger prefix to preserve Git's exact graph layout; it is not a
persistent revision walker. Search covers loaded commits. Very long sessions
still retain metadata for the commits they have loaded.

In three alternating real-terminal launches at 140×40, time to the first commit
hash was **650 ms median with the old snapshot loader** and **172 ms with the
independent history loader**. Both used the same already-indexed clone; the old
loader requested 300 commits and the new one requested 1,000. These numbers
include launch, terminal initialization, Git queries and presentation. They are
not a benchmark against GitUI, and do not measure loading the entire history.

Raw queries and all terminal samples are in
[benchmarks/llvm-history-loading.json](benchmarks/llvm-history-loading.json).
First launches, concurrent machine activity and filesystem caches affect the
samples; these local measurements do not establish a universal speedup.

GitUI's current implementation uses a persistent native revision walk, batches
3,000 commit IDs, and loads display metadata for a window around the cursor.
It also avoids starting another walk while the head is unchanged. The native
unfiltered walker uses commit-time ordering; the UI still lists branching
visualization as roadmap work. See its
[revision loader](https://github.com/gitui-org/gitui/blob/d7214eccb3cac52ce3f8d9ca18f218671821eeee/asyncgit/src/revlog.rs),
[walker](https://github.com/gitui-org/gitui/blob/d7214eccb3cac52ce3f8d9ca18f218671821eeee/asyncgit/src/sync/logwalker.rs),
and [visible commit metadata](https://github.com/gitui-org/gitui/blob/d7214eccb3cac52ce3f8d9ca18f218671821eeee/src/components/commitlist.rs).

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
  -o ../rustifiedgit/target/benchmark-go-lazygit .
cd ../rustifiedgit
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
./target/release/rustifiedgit --benchmark --iterations 20 -p ../pd_lazygit
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
