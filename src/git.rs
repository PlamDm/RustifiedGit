use crate::{
    config::GuiConfig,
    model::{
        Commit, CommitDetails, CommitInspection, FileEntry, Preview, PreviewTarget, RefKind,
        Reference, Snapshot, Stash, Worktree,
    },
    process::{self, Cancellation, CommandSpec, Output, os_path},
};
use anyhow::{Context, Result, bail};
use std::{
    collections::{BTreeMap, HashSet},
    ffi::OsString,
    fs,
    path::{Component, Path, PathBuf},
    sync::{
        Arc, Mutex,
        mpsc::{SyncSender, TrySendError},
    },
    thread,
    time::{Duration, Instant, SystemTime},
};

const READ_LIMIT: usize = 64 * 1024 * 1024;
const READ_TIMEOUT: Duration = Duration::from_secs(60);
pub const HISTORY_READ_LIMIT: usize = 512 * 1024 * 1024;
const HISTORY_READ_TIMEOUT: Duration = Duration::from_secs(180);
pub const HISTORY_PAGE_SIZE: usize = 1000;
type IndexStamp = (Option<SystemTime>, u64);
type TrackedCache = Arc<Mutex<Option<(IndexStamp, Vec<PathBuf>)>>>;
type TreeCache = Arc<Mutex<Option<(String, Vec<FileEntry>)>>>;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReadOptions {
    pub ignored: bool,
    pub history_path: Option<PathBuf>,
    pub history_directory: bool,
    pub reflog: bool,
    pub all_branches: bool,
    pub commit_limit: usize,
    pub graph: bool,
    pub visible_refs: Option<Vec<String>>,
}

#[derive(Clone)]
pub struct Repository {
    pub root: PathBuf,
    pub git_dir: PathBuf,
    tracked: TrackedCache,
    tree: TreeCache,
}

#[derive(Clone, Debug)]
pub enum Step {
    Command(CommandSpec),
    DeleteUntracked(PathBuf),
    AppendIgnore(PathBuf),
}

#[derive(Clone, Debug)]
pub struct Operation {
    pub label: String,
    pub steps: Vec<Step>,
}

impl Operation {
    pub fn git(args: impl IntoIterator<Item = impl Into<OsString>>) -> Self {
        let mut args: Vec<OsString> = args.into_iter().map(Into::into).collect();
        // Checkout progress normally requires a terminal. Managed commands use
        // pipes, including every local/remote/tag/create/previous-branch switch.
        if args.first().is_some_and(|arg| arg == "switch")
            && !args
                .iter()
                .take_while(|arg| *arg != "--")
                .any(|arg| arg == "--progress" || arg == "--no-progress")
        {
            args.insert(1, "--progress".into());
        }
        let command = CommandSpec::git(args);
        Self {
            label: command.label.clone(),
            steps: vec![Step::Command(command)],
        }
    }
    pub fn shell(command: String) -> Self {
        Self {
            label: command.clone(),
            steps: vec![Step::Command(CommandSpec::shell(command))],
        }
    }
    pub fn patch(data: Vec<u8>, staged: bool, reverse: bool) -> Self {
        let mut args = vec![
            "apply",
            "--recount",
            "--unidiff-zero",
            "--whitespace=nowarn",
        ];
        if staged {
            args.push("--cached");
        }
        if reverse {
            args.push("--reverse");
        }
        let mut command = CommandSpec::git(args);
        command.stdin = Some(data);
        Self {
            label: command.label.clone(),
            steps: vec![Step::Command(command)],
        }
    }
}

fn without_newline(bytes: &[u8]) -> &[u8] {
    bytes.strip_suffix(b"\n").unwrap_or(bytes)
}

fn history_output_error(output: &Output, requested: usize) -> Option<anyhow::Error> {
    // Overflow terminates Git, so its exit code is also unsuccessful and stderr
    // is often empty. Report the useful cause before examining that exit code.
    if output.truncated {
        return Some(anyhow::anyhow!(
            "Commit history exceeded the 512 MiB history read limit while loading up to {requested} commits. Press L in the graph to choose a smaller initial/page size, or select fewer branches."
        ));
    }
    if output.code == 0 {
        return None;
    }
    let details = String::from_utf8_lossy(if output.stderr.is_empty() {
        &output.stdout
    } else {
        &output.stderr
    });
    let details = details.trim();
    Some(if details.is_empty() {
        anyhow::anyhow!(
            "Git could not load up to {requested} commits (exit code {}); no error details were returned.",
            output.code
        )
    } else {
        anyhow::anyhow!("Git could not load up to {requested} commits: {details}")
    })
}

impl Repository {
    pub fn discover(path: &Path) -> Result<Self> {
        let cancel = Cancellation::default();
        let query = |arg: &str| -> Result<PathBuf> {
            let output = process::run(
                path,
                &CommandSpec::git(["rev-parse", arg]),
                &cancel,
                1024 * 1024,
                None,
                READ_TIMEOUT,
            )?
            .success()?;
            Ok(PathBuf::from(os_path(without_newline(&output.stdout))))
        };
        let root = query("--show-toplevel")
            .context("Open a Git working tree with -p PATH, or run git init first")?;
        let git_dir = query("--absolute-git-dir")?;
        Ok(Self {
            root,
            git_dir,
            tracked: Arc::new(Mutex::new(None)),
            tree: Arc::new(Mutex::new(None)),
        })
    }

    pub fn read(
        &self,
        args: impl IntoIterator<Item = impl Into<OsString>>,
        cancel: &Cancellation,
    ) -> Result<Output> {
        let result = process::run(
            &self.root,
            &CommandSpec::git(args),
            cancel,
            READ_LIMIT,
            None,
            READ_TIMEOUT,
        )?;
        if result.truncated {
            bail!("Git output exceeded 64 MiB; narrow the selection");
        }
        result.success()
    }

    fn tracked_files(&self, cancel: &Cancellation) -> Result<Vec<PathBuf>> {
        let metadata = fs::metadata(self.git_dir.join("index")).ok();
        let stamp = metadata
            .map(|metadata| (metadata.modified().ok(), metadata.len()))
            .unwrap_or((None, 0));
        if let Some((cached_stamp, paths)) = self.tracked.lock().unwrap().as_ref() {
            if *cached_stamp == stamp {
                return Ok(paths.clone());
            }
        }
        let output = self.read(["ls-files", "--cached", "-z"], cancel)?;
        let mut paths: Vec<_> = output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
            .map(|path| PathBuf::from(os_path(path)))
            .collect();
        paths.sort();
        paths.dedup();
        *self.tracked.lock().unwrap() = Some((stamp, paths.clone()));
        Ok(paths)
    }

    /// The committed tree is independent of the index and working directory.
    /// Cache one immutable tree so switching tabs never repeats a large listing.
    pub fn tree_files(&self, oid: &str, cancel: &Cancellation) -> Result<Vec<FileEntry>> {
        cancel.check()?;
        if oid.is_empty() {
            return Ok(Vec::new());
        }
        if let Some((cached_oid, files)) = self.tree.lock().unwrap().as_ref() {
            if cached_oid == oid {
                return Ok(files.clone());
            }
        }
        if !matches!(oid.len(), 40 | 64) || !oid.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            bail!("Expected an immutable commit object ID for the HEAD tree");
        }
        let output = self.read(["ls-tree", "-r", "--name-only", "-z", oid, "--"], cancel)?;
        let files = output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
            .map(|path| FileEntry::clean(PathBuf::from(os_path(path))))
            .collect::<Vec<_>>();
        cancel.check()?;
        *self.tree.lock().unwrap() = Some((oid.to_owned(), files.clone()));
        Ok(files)
    }

    fn tree_file_preview(
        &self,
        path: &Path,
        oid: &str,
        explicit: bool,
        settings: &GuiConfig,
        cancel: &Cancellation,
    ) -> Result<Preview> {
        if !matches!(oid.len(), 40 | 64) || !oid.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            bail!("Expected an immutable commit object ID for the HEAD file");
        }
        let title = format!("HEAD: {}", path.to_string_lossy());
        let mut object = OsString::from(format!("{oid}:"));
        object.push(path);
        if !explicit && settings.diff_preview_max_file_bytes > 0 {
            let size = self.read(
                [OsString::from("cat-file"), "-s".into(), object.clone()],
                cancel,
            )?;
            let size = size
                .text()
                .trim()
                .parse::<usize>()
                .context("Invalid blob size")?;
            if size > settings.diff_preview_max_file_bytes {
                return Ok(Preview {
                    title,
                    text: format!(
                        "Automatic preview skipped: file exceeds {} bytes.\nPress 0 or Enter to open its committed contents.",
                        settings.diff_preview_max_file_bytes
                    ),
                    suppressed: true,
                    ..Preview::default()
                });
            }
        }
        let limit = if !explicit && settings.diff_preview_max_bytes > 0 {
            settings.diff_preview_max_bytes
        } else {
            READ_LIMIT
        };
        let output = process::run(
            &self.root,
            &CommandSpec::git([OsString::from("cat-file"), "-p".into(), object]),
            cancel,
            limit,
            None,
            READ_TIMEOUT,
        )?
        .success()?;
        if output.truncated {
            return Ok(Preview {
                title,
                text: if explicit {
                    "Committed file exceeds the 64 MiB viewer limit.".into()
                } else {
                    format!(
                        "Automatic preview skipped: output exceeds {limit} bytes.\nPress 0 or Enter to open its committed contents."
                    )
                },
                suppressed: true,
                ..Preview::default()
            });
        }
        Ok(Preview {
            title,
            text: if output.stdout.contains(&0) {
                "Binary file in HEAD.".into()
            } else {
                String::from_utf8_lossy(&output.stdout).into_owned()
            },
            // Contents may themselves contain patch syntax; never expose them as a patch.
            patch: Vec::new(),
            ..Preview::default()
        })
    }

    pub fn files(&self, ignored: bool, cancel: &Cancellation) -> Result<(Vec<FileEntry>, String)> {
        let setting = process::run(
            &self.root,
            &CommandSpec::git(["config", "--get", "status.showUntrackedFiles"]),
            cancel,
            4096,
            None,
            READ_TIMEOUT,
        )?;
        let show_untracked = setting.code != 0 || !matches!(setting.text().trim(), "no" | "false");
        let args = [
            "status",
            "--porcelain=v1",
            "--branch",
            "-z",
            if show_untracked {
                "--untracked-files=all"
            } else {
                "--untracked-files=no"
            },
        ];
        let (status, tracked) = thread::scope(|scope| {
            let tracked = scope.spawn(|| self.tracked_files(cancel));
            let status = self.read(args, cancel);
            (status, tracked.join().unwrap())
        });
        let (changed, branch) = parse_status(&status?.stdout)?;
        let mut files: BTreeMap<PathBuf, FileEntry> = tracked?
            .into_iter()
            .map(|path| (path.clone(), FileEntry::clean(path)))
            .collect();
        for file in changed {
            if let Some(old) = &file.old_path {
                files.remove(old);
            }
            files.insert(file.path.clone(), file);
        }
        if ignored {
            let output = self.read(
                [
                    "ls-files",
                    "--others",
                    "--ignored",
                    "--exclude-standard",
                    "-z",
                ],
                cancel,
            )?;
            for path in output
                .stdout
                .split(|byte| *byte == 0)
                .filter(|path| !path.is_empty())
            {
                let path = PathBuf::from(os_path(path));
                files.entry(path.clone()).or_insert(FileEntry {
                    path,
                    old_path: None,
                    index: b'!',
                    worktree: b'!',
                });
            }
        }
        Ok((files.into_values().collect(), branch))
    }

    pub fn references(&self, cancel: &Cancellation) -> Result<Vec<Reference>> {
        let output = self.read(
            [
                "for-each-ref",
                "--sort=-committerdate",
                "--format=%(refname)%00%(objectname)%00%(upstream:short)%00%(HEAD)%00%(upstream:track)%00",
                "refs/heads",
                "refs/remotes",
                "refs/tags",
            ],
            cancel,
        )?;
        let fields: Vec<_> = output.stdout.split(|byte| *byte == 0).collect();
        let mut refs = Vec::new();
        for fields in fields.chunks_exact(5) {
            let full_name = String::from_utf8_lossy(fields[0])
                .trim_start_matches('\n')
                .to_owned();
            let (kind, name) = if let Some(name) = full_name.strip_prefix("refs/heads/") {
                (RefKind::Branch, name)
            } else if let Some(name) = full_name.strip_prefix("refs/remotes/") {
                (RefKind::Remote, name)
            } else if let Some(name) = full_name.strip_prefix("refs/tags/") {
                (RefKind::Tag, name)
            } else {
                continue;
            };
            let upstream = String::from_utf8_lossy(fields[2]).into_owned();
            let tracking = String::from_utf8_lossy(fields[4]);
            let counts = if upstream.is_empty() || tracking == "[gone]" {
                None
            } else {
                let mut counts = (0, 0);
                for part in tracking.trim_matches(['[', ']']).split(", ") {
                    if let Some(number) = part.strip_prefix("ahead ") {
                        counts.0 = number.parse().context("Invalid upstream ahead count")?;
                    } else if let Some(number) = part.strip_prefix("behind ") {
                        counts.1 = number.parse().context("Invalid upstream behind count")?;
                    }
                }
                Some(counts)
            };
            refs.push(Reference {
                name: name.to_owned(),
                kind,
                full_name,
                oid: String::from_utf8_lossy(fields[1]).into_owned(),
                upstream,
                ahead: counts.map(|counts| counts.0),
                behind: counts.map(|counts| counts.1),
                current: fields[3] == b"*",
            });
        }
        Ok(refs)
    }

    pub fn commits(&self, options: &ReadOptions, cancel: &Cancellation) -> Result<Vec<Commit>> {
        if !options.reflog && options.visible_refs.as_ref().is_some_and(Vec::is_empty) {
            return Ok(Vec::new());
        }
        let mut args: Vec<OsString> = vec![if options.reflog { "reflog" } else { "log" }.into()];
        if options.reflog {
            args.push("show".into());
        }
        args.extend([
            "-z".into(),
            format!(
                "--max-count={}",
                if options.commit_limit == 0 {
                    HISTORY_PAGE_SIZE
                } else {
                    options.commit_limit
                }
            )
            .into(),
            "--format=%x00%H%x00%h%x00%s%x00%an%x00%aI%x00%D%x00%P%x00%cn%x00%cI%x00".into(),
        ]);
        if options.graph && !options.reflog {
            args.extend([
                "--graph".into(),
                "--topo-order".into(),
                "--color=always".into(),
            ]);
        }
        if !options.reflog && options.visible_refs.is_some() {
            args.extend(["--ignore-missing".into(), "--stdin".into()]);
        } else if options.all_branches && !options.reflog {
            args.push("--all".into());
        }
        if let Some(path) = &options.history_path {
            if !options.history_directory && !options.reflog {
                args.push("--follow".into());
            }
            args.extend(["--".into(), path.as_os_str().to_owned()]);
        }
        let mut command = CommandSpec::git(args);
        if !options.reflog {
            if let Some(references) = &options.visible_refs {
                if references.iter().any(|name| {
                    name.contains(['\n', '\r', '\0'])
                        || !(name.starts_with("refs/")
                            || name == "HEAD"
                            || (name.len() >= 40
                                && name.bytes().all(|byte| byte.is_ascii_hexdigit())))
                }) {
                    bail!("Invalid graph reference selection");
                }
                command.stdin = Some(format!("{}\n", references.join("\n")).into_bytes());
            }
        }
        let requested = if options.commit_limit == 0 {
            HISTORY_PAGE_SIZE
        } else {
            options.commit_limit
        };
        let output = process::run(
            &self.root,
            &command,
            cancel,
            HISTORY_READ_LIMIT,
            None,
            HISTORY_READ_TIMEOUT,
        )
        .with_context(|| format!("Loading up to {requested} commits failed"))?;
        if let Some(error) = history_output_error(&output, requested) {
            if output.truncated {
                return Err(error);
            }
            let head = process::run(
                &self.root,
                &CommandSpec::git(["rev-parse", "--verify", "HEAD"]),
                cancel,
                4096,
                None,
                READ_TIMEOUT,
            )?;
            if head.code != 0 {
                return Ok(Vec::new());
            }
            return Err(error);
        }
        let mut commits = Vec::new();
        for (index, commit) in commit_records(&output.stdout).enumerate() {
            if index % 1024 == 0 {
                cancel.check()?;
            }
            commits.push(commit);
        }
        cancel.check()?;
        Ok(commits)
    }

    pub fn stashes(&self, cancel: &Cancellation) -> Result<Vec<Stash>> {
        let output = self.read(["stash", "list", "-z", "--format=%gd%x00%H%x00%s"], cancel)?;
        Ok(output
            .stdout
            .split(|byte| *byte == 0)
            .collect::<Vec<_>>()
            .chunks_exact(3)
            .map(|fields| Stash {
                reference: String::from_utf8_lossy(fields[0]).into_owned(),
                oid: String::from_utf8_lossy(fields[1]).into_owned(),
                subject: String::from_utf8_lossy(fields[2]).into_owned(),
            })
            .collect())
    }

    pub fn worktrees(&self, cancel: &Cancellation) -> Result<Vec<Worktree>> {
        let output = self.read(["worktree", "list", "--porcelain", "-z"], cancel)?;
        let mut result = Vec::new();
        let mut current: Option<Worktree> = None;
        for field in output.stdout.split(|byte| *byte == 0) {
            if let Some(path) = field.strip_prefix(b"worktree ") {
                if let Some(worktree) = current.take() {
                    result.push(worktree);
                }
                current = Some(Worktree {
                    path: PathBuf::from(os_path(path)),
                    ..Worktree::default()
                });
            } else if let Some(worktree) = &mut current {
                if let Some(oid) = field.strip_prefix(b"HEAD ") {
                    worktree.oid = String::from_utf8_lossy(oid).into_owned();
                }
                if let Some(branch) = field.strip_prefix(b"branch refs/heads/") {
                    worktree.branch = String::from_utf8_lossy(branch).into_owned();
                }
                worktree.locked |= field.starts_with(b"locked");
            }
        }
        if let Some(worktree) = current {
            result.push(worktree);
        }
        Ok(result)
    }

    pub fn snapshot(&self, options: &ReadOptions, cancel: &Cancellation) -> Result<Snapshot> {
        self.snapshot_inner(options, cancel, true)
    }

    pub fn status_snapshot(
        &self,
        options: &ReadOptions,
        cancel: &Cancellation,
    ) -> Result<Snapshot> {
        self.snapshot_inner(options, cancel, false)
    }

    pub fn ref_state(&self, cancel: &Cancellation) -> Result<(Vec<Reference>, String)> {
        let references = self.references(cancel)?;
        let head = if let Some(reference) = references.iter().find(|reference| reference.current) {
            reference.oid.clone()
        } else {
            let output = process::run(
                &self.root,
                &CommandSpec::git(["rev-parse", "--verify", "HEAD"]),
                cancel,
                4096,
                None,
                READ_TIMEOUT,
            )?;
            if output.code == 0 {
                output.text().trim().to_owned()
            } else {
                String::new()
            }
        };
        Ok((references, head))
    }

    fn snapshot_inner(
        &self,
        options: &ReadOptions,
        cancel: &Cancellation,
        include_history: bool,
    ) -> Result<Snapshot> {
        let started = Instant::now();
        let (files, refs, commits, stashes, worktrees) = thread::scope(|scope| {
            let files = scope.spawn(|| self.files(options.ignored, cancel));
            let refs = scope.spawn(|| self.references(cancel));
            let commits = scope.spawn(|| {
                if include_history {
                    self.commits(options, cancel)
                } else {
                    Ok(Vec::new())
                }
            });
            let stashes = scope.spawn(|| self.stashes(cancel));
            let worktrees = self.worktrees(cancel);
            (
                files.join().unwrap(),
                refs.join().unwrap(),
                commits.join().unwrap(),
                stashes.join().unwrap(),
                worktrees,
            )
        });
        let (files, branch_description) = files?;
        let references = refs?;
        let current = references.iter().find(|reference| reference.current);
        let head = match current {
            Some(reference) => reference.oid.clone(),
            None => {
                let output = process::run(
                    &self.root,
                    &CommandSpec::git(["rev-parse", "--verify", "HEAD"]),
                    cancel,
                    4096,
                    None,
                    READ_TIMEOUT,
                )?;
                if output.code == 0 {
                    output.text().trim().to_owned()
                } else {
                    String::new()
                }
            }
        };
        let upstream = current
            .map(|reference| reference.upstream.clone())
            .unwrap_or_default();
        let ahead = current.and_then(|reference| reference.ahead);
        let behind = current.and_then(|reference| reference.behind);
        let operation = [
            ("rebase-merge", "REBASING"),
            ("rebase-apply", "REBASING"),
            ("MERGE_HEAD", "MERGING"),
            ("CHERRY_PICK_HEAD", "CHERRY-PICKING"),
            ("REVERT_HEAD", "REVERTING"),
            ("BISECT_LOG", "BISECTING"),
        ]
        .iter()
        .find(|(path, _)| self.git_dir.join(path).exists())
        .map(|(_, name)| name.to_string())
        .unwrap_or_default();
        Ok(Snapshot {
            files,
            references,
            commits: Arc::new(commits?),
            stashes: stashes?,
            worktrees: worktrees?,
            branch: branch_description,
            head,
            upstream,
            ahead,
            behind,
            operation,
            elapsed_ms: started.elapsed().as_millis(),
        })
    }

    fn file_too_large(
        &self,
        path: &Path,
        old_path: Option<&Path>,
        commit: Option<&str>,
        maximum: usize,
        cancel: &Cancellation,
    ) -> Result<bool> {
        if maximum == 0 {
            return Ok(false);
        }
        let paths: Vec<_> = std::iter::once(path).chain(old_path).collect();
        if commit.is_none() {
            for path in &paths {
                if fs::symlink_metadata(self.root.join(path))
                    .is_ok_and(|metadata| metadata.is_file() && metadata.len() > maximum as u64)
                {
                    return Ok(true);
                }
            }
            let mut args: Vec<OsString> = vec![
                "ls-files".into(),
                "--stage".into(),
                "-z".into(),
                "--".into(),
            ];
            args.extend(paths.iter().map(|path| path.as_os_str().to_owned()));
            let output = self.read(args, cancel)?;
            let mut seen = HashSet::new();
            let mut objects = String::new();
            for entry in output
                .stdout
                .split(|byte| *byte == 0)
                .filter(|entry| !entry.is_empty())
            {
                let header = entry
                    .split(|byte| *byte == b'\t')
                    .next()
                    .unwrap_or_default();
                let fields: Vec<_> = header.split(|byte| *byte == b' ').collect();
                if fields.len() == 3 && fields[0] != b"160000" && seen.insert(fields[1].to_vec()) {
                    objects.push_str(&String::from_utf8_lossy(fields[1]));
                    objects.push('\n');
                }
            }
            if !objects.is_empty() {
                let mut command = CommandSpec::git(["cat-file", "--batch-check=%(objectsize)"]);
                command.stdin = Some(objects.into_bytes());
                let output =
                    process::run(&self.root, &command, cancel, READ_LIMIT, None, READ_TIMEOUT)?
                        .success()?;
                if output
                    .text()
                    .lines()
                    .any(|size| size.parse::<usize>().is_ok_and(|size| size > maximum))
                {
                    return Ok(true);
                }
            }
        }
        {
            let refs = if let Some(commit) = commit {
                vec![commit.to_owned(), format!("{commit}^")]
            } else {
                vec!["HEAD".to_owned()]
            };
            for reference in refs {
                let mut args: Vec<OsString> = vec![
                    "ls-tree".into(),
                    "-r".into(),
                    "-l".into(),
                    "-z".into(),
                    reference.into(),
                    "--".into(),
                ];
                args.extend(paths.iter().map(|path| path.as_os_str().to_owned()));
                let output = process::run(
                    &self.root,
                    &CommandSpec::git(args),
                    cancel,
                    READ_LIMIT,
                    None,
                    READ_TIMEOUT,
                )?;
                if output.code != 0 {
                    continue;
                }
                for entry in output.stdout.split(|byte| *byte == 0) {
                    let header = entry
                        .split(|byte| *byte == b'\t')
                        .next()
                        .unwrap_or_default();
                    if String::from_utf8_lossy(header)
                        .split_whitespace()
                        .nth(3)
                        .and_then(|size| size.parse::<usize>().ok())
                        .is_some_and(|size| size > maximum)
                    {
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    }

    pub fn preview(
        &self,
        target: &PreviewTarget,
        explicit: bool,
        settings: &GuiConfig,
        context: usize,
        ignore_whitespace: bool,
        cancel: &Cancellation,
    ) -> Result<Preview> {
        let mut args: Vec<OsString> = Vec::new();
        let mut title = String::new();
        let mut staged = false;
        let file = matches!(target, PreviewTarget::File { .. });
        if let PreviewTarget::File {
            path,
            old_path,
            commit,
            ..
        } = target
        {
            title = path.to_string_lossy().into_owned();
            if !explicit
                && self.file_too_large(
                    path,
                    old_path.as_deref(),
                    commit.as_deref(),
                    settings.diff_preview_max_file_bytes,
                    cancel,
                )?
            {
                return Ok(Preview {
                    title,
                    text: format!(
                        "Automatic preview skipped: a file version exceeds {} bytes.\nPress 0 or Enter to open it, or Ctrl+t for the external diff tool.",
                        settings.diff_preview_max_file_bytes
                    ),
                    suppressed: true,
                    ..Preview::default()
                });
            }
        }
        match target {
            PreviewTarget::TreeFile { path, oid } => {
                return self.tree_file_preview(path, oid, explicit, settings, cancel);
            }
            PreviewTarget::File {
                path,
                old_path,
                untracked,
                ignored,
                staged: cached,
                commit,
            } => {
                staged = *cached;
                if let Some(commit) = commit {
                    args.extend([
                        "show".into(),
                        "--format=".into(),
                        "--root".into(),
                        "--first-parent".into(),
                        commit.into(),
                    ]);
                } else {
                    args.push("diff".into());
                    if *untracked || *ignored {
                        #[cfg(windows)]
                        let null = "NUL";
                        #[cfg(not(windows))]
                        let null = "/dev/null";
                        args.extend([
                            "--no-index".into(),
                            "--no-color".into(),
                            "--no-ext-diff".into(),
                            format!("--unified={context}").into(),
                            "--".into(),
                            null.into(),
                            path.as_os_str().to_owned(),
                        ]);
                    } else if *cached {
                        args.push("--cached".into());
                    }
                }
                if !untracked && !ignored || commit.is_some() {
                    args.extend([
                        "--no-color".into(),
                        "--no-ext-diff".into(),
                        "--no-textconv".into(),
                        format!("--unified={context}").into(),
                    ]);
                    if ignore_whitespace {
                        args.push("--ignore-all-space".into());
                    }
                    args.push("--".into());
                    args.push(path.as_os_str().to_owned());
                    if let Some(old_path) = old_path {
                        args.push(old_path.as_os_str().to_owned());
                    }
                }
            }
            PreviewTarget::Commit(oid) => {
                title = format!("Commit {oid}");
                args.extend([
                    "show".into(),
                    "--stat".into(),
                    "--patch".into(),
                    "--no-color".into(),
                    "--no-ext-diff".into(),
                    oid.into(),
                    "--".into(),
                ]);
            }
            PreviewTarget::Reference(reference) => {
                title = reference.clone();
                args.extend([
                    "log".into(),
                    "--graph".into(),
                    "--oneline".into(),
                    "--decorate".into(),
                    "--max-count=300".into(),
                    reference.into(),
                    "--".into(),
                ]);
            }
            PreviewTarget::Stash(reference) => {
                title = reference.clone();
                args.extend([
                    "stash".into(),
                    "show".into(),
                    "--patch".into(),
                    "--include-untracked".into(),
                    "--no-color".into(),
                    reference.into(),
                    "--".into(),
                ]);
            }
            PreviewTarget::Status => {
                title = "Repository status".into();
                args.extend(["status".into(), "--short".into(), "--branch".into()]);
            }
        }
        let limit = if file && !explicit && settings.diff_preview_max_bytes > 0 {
            settings.diff_preview_max_bytes
        } else {
            usize::MAX
        };
        let output = process::run(
            &self.root,
            &CommandSpec::git(args),
            cancel,
            limit,
            None,
            READ_TIMEOUT,
        )?;
        if output.truncated {
            return Ok(Preview {
                title,
                text: format!(
                    "Automatic preview skipped: output exceeds {limit} bytes.\nPress 0 or Enter to open it, or Ctrl+t for the external diff tool."
                ),
                suppressed: true,
                staged,
                ..Preview::default()
            });
        }
        if output.code != 0
            && !(output.code == 1
                && matches!(
                    target,
                    PreviewTarget::File {
                        untracked: true,
                        ..
                    } | PreviewTarget::File { ignored: true, .. }
                ))
        {
            output.success()?;
            unreachable!();
        }
        let patch = output.stdout;
        let text = if patch.is_empty() {
            if let PreviewTarget::File {
                path, commit: None, ..
            } = target
            {
                let metadata = fs::metadata(self.root.join(path));
                if metadata.is_ok_and(|metadata| metadata.is_file()) {
                    use std::io::Read;
                    let mut contents = Vec::new();
                    fs::File::open(self.root.join(path))?
                        .take(if limit == usize::MAX {
                            u64::MAX
                        } else {
                            limit as u64 + 1
                        })
                        .read_to_end(&mut contents)?;
                    if contents.len() > limit {
                        return Ok(Preview { title, text: "File content exceeds the automatic preview limit. Press 0 to open it.".into(), suppressed: true, staged, ..Preview::default() });
                    }
                    if contents.contains(&0) {
                        "Binary file; open with o or e.".into()
                    } else {
                        String::from_utf8_lossy(&contents).into_owned()
                    }
                } else {
                    "No diff for this selection.".into()
                }
            } else {
                "No diff for this selection.".into()
            }
        } else {
            String::from_utf8_lossy(&patch).into_owned()
        };
        Ok(Preview {
            title,
            text,
            patch,
            staged,
            suppressed: false,
        })
    }

    pub fn commit_files(&self, oid: &str, cancel: &Cancellation) -> Result<Vec<FileEntry>> {
        let output = self.read(
            [
                "diff-tree",
                "--root",
                "--first-parent",
                "-m",
                "--no-commit-id",
                "--name-status",
                "-r",
                "-z",
                "-M",
                oid,
                "--",
            ],
            cancel,
        )?;
        let fields: Vec<_> = output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|field| !field.is_empty())
            .collect();
        let mut result = Vec::new();
        let mut index = 0;
        while index + 1 < fields.len() {
            let status = fields[index].first().copied().unwrap_or(b' ');
            let path = fields[index + 1];
            index += 2;
            let (old_path, path) = if matches!(status, b'R' | b'C') && index < fields.len() {
                let new = fields[index];
                index += 1;
                (Some(PathBuf::from(os_path(path))), new)
            } else {
                (None, path)
            };
            result.push(FileEntry {
                path: PathBuf::from(os_path(path)),
                old_path,
                index: status,
                worktree: b' ',
            });
        }
        Ok(result)
    }

    pub fn inspect_commit(&self, oid: &str, cancel: &Cancellation) -> Result<CommitInspection> {
        let (files, details) = thread::scope(|scope| {
            let files = scope.spawn(|| self.commit_files(oid, cancel));
            let output = self.read(
                [
                    "show",
                    "--no-patch",
                    "--format=%H%x00%B%x00%an%x00%ae%x00%cn%x00%ce%x00%cI%x00%P",
                    oid,
                    "--",
                ],
                cancel,
            );
            (files.join().unwrap(), output)
        });
        let details = details?;
        let fields: Vec<_> = without_newline(&details.stdout)
            .split(|byte| *byte == 0)
            .collect();
        if fields.len() != 8 {
            bail!("Invalid commit detail response");
        }
        let field = |index| String::from_utf8_lossy(fields[index]).into_owned();
        Ok(CommitInspection {
            files: files?,
            details: CommitDetails {
                oid: field(0),
                message: field(1).trim_end().to_owned(),
                author: field(2),
                author_email: field(3),
                committer: field(4),
                committer_email: field(5),
                committed_at: field(6),
                parents: field(7).split_whitespace().map(str::to_owned).collect(),
            },
        })
    }

    pub fn execute(
        &self,
        operation: &Operation,
        cancel: &Cancellation,
        stream: Option<SyncSender<Vec<u8>>>,
    ) -> Result<String> {
        let mut summary = String::new();
        let log_step = |message: String| -> Result<()> {
            let Some(stream) = &stream else {
                return Ok(());
            };
            let mut message = message.into_bytes();
            loop {
                cancel.check()?;
                match stream.try_send(message) {
                    Ok(()) => return Ok(()),
                    Err(TrySendError::Full(pending)) => {
                        message = pending;
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(TrySendError::Disconnected(_)) => bail!("Command output receiver closed"),
                }
            }
        };
        for step in &operation.steps {
            cancel.check()?;
            match step {
                Step::Command(command) => {
                    log_step(format!("> {}\n", command.label))?;
                    let output = process::run(
                        &self.root,
                        command,
                        cancel,
                        READ_LIMIT,
                        stream.clone(),
                        Duration::from_secs(60 * 60),
                    )?;
                    let truncated = output.truncated;
                    let output = output.success()?;
                    summary.push_str(&output.text());
                    summary.push_str(&String::from_utf8_lossy(&output.stderr));
                    if truncated {
                        summary.push_str("\nOutput reached 64 MiB; command was stopped.\n");
                    }
                }
                Step::DeleteUntracked(path) => {
                    validate_path(path)?;
                    log_step(format!("> Remove untracked path {path:?}\n"))?;
                    // Revalidate immediately before deletion; an external process may have staged it.
                    let output = process::run(
                        &self.root,
                        &CommandSpec::git([
                            OsString::from("ls-files"),
                            "--error-unmatch".into(),
                            "--".into(),
                            path.as_os_str().to_owned(),
                        ]),
                        cancel,
                        READ_LIMIT,
                        None,
                        READ_TIMEOUT,
                    )?;
                    if output.code == 0 {
                        bail!("Refusing to delete a now-tracked file: {}", path.display());
                    }
                    if output.code != 1 {
                        output.success()?;
                    }
                    let absolute = self.root.join(path);
                    let metadata = fs::symlink_metadata(&absolute)?;
                    if metadata.is_dir() {
                        bail!(
                            "Refusing recursive directory deletion; select individual untracked files"
                        );
                    }
                    fs::remove_file(absolute)?;
                }
                Step::AppendIgnore(path) => {
                    validate_path(path)?;
                    log_step(format!("> Add {path:?} to .gitignore\n"))?;
                    use std::io::Write;
                    let text = path
                        .to_str()
                        .context("Cannot add a non-UTF-8 path to .gitignore automatically")?;
                    if text.contains(['\n', '\r']) {
                        bail!("Cannot add a newline-containing name to .gitignore");
                    }
                    let escaped = text
                        .replace('\\', "\\\\")
                        .replace('*', "\\*")
                        .replace('?', "\\?")
                        .replace('[', "\\[")
                        .replace(' ', "\\ ");
                    let mut file = fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(self.root.join(".gitignore"))?;
                    writeln!(file, "\n/{escaped}")?;
                }
            }
        }
        Ok(summary)
    }
}

pub fn validate_path(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        bail!("Invalid repository-relative path: {}", path.display());
    }
    Ok(())
}

pub fn stage_operation(files: &[&FileEntry], unstage: bool, has_head: bool) -> Result<Operation> {
    let mut paths = Vec::new();
    for file in files
        .iter()
        .filter(|file| file.changed() && !file.ignored())
    {
        validate_path(&file.path)?;
        paths.push(file.path.as_os_str().to_owned());
        if let Some(old) = &file.old_path {
            validate_path(old)?;
            if unstage || !matches!(file.index, b'R' | b'C') {
                paths.push(old.as_os_str().to_owned());
            }
        }
    }
    if paths.is_empty() {
        bail!("No eligible changed files in this selection");
    }
    let mut args: Vec<OsString> = if unstage {
        if has_head {
            vec!["restore".into(), "--staged".into()]
        } else {
            vec![
                "rm".into(),
                "--cached".into(),
                "--ignore-unmatch".into(),
                "-r".into(),
            ]
        }
    } else {
        vec!["add".into(), "--all".into()]
    };
    args.push("--".into());
    args.extend(paths);
    Ok(Operation::git(args))
}

pub fn discard_operation(files: &[&FileEntry], has_head: bool) -> Result<Operation> {
    let mut steps = Vec::new();
    let mut tracked = Vec::new();
    for file in files
        .iter()
        .filter(|file| file.changed() && !file.ignored())
    {
        validate_path(&file.path)?;
        if file.untracked() {
            steps.push(Step::DeleteUntracked(file.path.clone()));
        } else {
            tracked.push(file.path.as_os_str().to_owned());
            if let Some(old) = &file.old_path {
                validate_path(old)?;
                tracked.push(old.as_os_str().to_owned());
            }
        }
    }
    if !tracked.is_empty() {
        if !has_head {
            bail!("An unborn repository has no HEAD to restore; unstage new files first");
        }
        let mut args: Vec<OsString> = vec![
            "restore".into(),
            "--source=HEAD".into(),
            "--staged".into(),
            "--worktree".into(),
            "--".into(),
        ];
        args.extend(tracked);
        steps.insert(0, Step::Command(CommandSpec::git(args)));
    }
    if steps.is_empty() {
        bail!("No changes to discard");
    }
    Ok(Operation {
        label: "Discard selected changes (index and working tree)".into(),
        steps,
    })
}

pub fn nuke_working_tree_operation() -> Operation {
    let commands = vec![
        CommandSpec::git([
            "submodule",
            "foreach",
            "--recursive",
            "git stash push --include-untracked",
        ]),
        CommandSpec::git(["submodule", "update", "--force", "--progress"]),
        CommandSpec::git(["reset", "--hard", "HEAD"]),
        CommandSpec::git(["clean", "-fd"]),
    ];
    Operation {
        label: "Nuke working tree: stash submodule changes, reset submodules and HEAD, clean untracked files (keeps ignored files)".into(),
        steps: commands.into_iter().map(Step::Command).collect(),
    }
}

pub fn pull_overwrite_operation() -> Operation {
    let commands = vec![
        // Resolve before fetching, then again after pruning: never reset to an
        // absent or guessed remote branch.
        CommandSpec::git(["rev-parse", "--verify", "@{upstream}"]),
        CommandSpec::git(["fetch", "--prune", "--progress"]),
        CommandSpec::git(["rev-parse", "--verify", "@{upstream}"]),
        CommandSpec::git([
            "submodule",
            "foreach",
            "--recursive",
            "git stash push --include-untracked",
        ]),
        CommandSpec::git(["reset", "--hard", "@{upstream}"]),
        CommandSpec::git([
            "submodule",
            "update",
            "--force",
            "--recursive",
            "--progress",
        ]),
    ];
    Operation {
        label: "Overwrite local branch with its fetched upstream: discard local-only commits and tracked changes; stash submodule changes (keeps unrelated untracked files)".into(),
        steps: commands.into_iter().map(Step::Command).collect(),
    }
}

pub fn reset_submodule_operation(path: &Path) -> Result<Operation> {
    validate_path(path)?;
    let stash = CommandSpec::git([
        OsString::from("-C"),
        path.as_os_str().to_owned(),
        "stash".into(),
        "push".into(),
        "--include-untracked".into(),
    ]);
    let reset = CommandSpec::git([
        OsString::from("submodule"),
        "update".into(),
        "--init".into(),
        "--force".into(),
        "--progress".into(),
        "--".into(),
        path.as_os_str().to_owned(),
    ]);
    Ok(Operation {
        label: format!(
            "Stash changes and reset submodule {} to its recorded commit",
            path.display()
        ),
        steps: vec![Step::Command(stash), Step::Command(reset)],
    })
}

pub fn parse_status(bytes: &[u8]) -> Result<(Vec<FileEntry>, String)> {
    let mut records = bytes
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty());
    let mut result = Vec::new();
    let mut branch = String::new();
    while let Some(record) = records.next() {
        if let Some(description) = record.strip_prefix(b"## ") {
            branch = String::from_utf8_lossy(description).into_owned();
            continue;
        }
        if record.len() < 4 || record[2] != b' ' {
            bail!("Malformed porcelain status record");
        }
        let old_path = if matches!(record[0], b'R' | b'C') || matches!(record[1], b'R' | b'C') {
            Some(PathBuf::from(os_path(
                records
                    .next()
                    .context("Rename record has no original path")?,
            )))
        } else {
            None
        };
        result.push(FileEntry {
            path: PathBuf::from(os_path(&record[3..])),
            old_path,
            index: record[0],
            worktree: record[1],
        });
    }
    Ok((result, branch))
}

pub fn parse_commits(bytes: &[u8]) -> Vec<Commit> {
    commit_records(bytes).collect()
}

fn commit_records(bytes: &[u8]) -> impl Iterator<Item = Commit> + '_ {
    // Keep only one record's field slices. A million commits have roughly
    // eleven million NUL fields, whose collected pointers alone cost 168 MiB.
    let mut fields = bytes.split(|byte| *byte == 0);
    let mut graph = Vec::new();
    std::iter::from_fn(move || {
        loop {
            let field = fields.next()?;
            if matches!(field.len(), 40 | 64) && field.iter().all(u8::is_ascii_hexdigit) {
                let mut record = [field; 9];
                for field in &mut record[1..] {
                    *field = fields.next()?;
                }
                let text = |offset: usize| String::from_utf8_lossy(record[offset]).into_owned();
                let graph_prefix = graph.pop().unwrap_or_default();
                return Some(Commit {
                    oid: text(0),
                    short: text(1),
                    subject: text(2),
                    author: text(3),
                    date: text(4),
                    committer: text(7),
                    committed_at: text(8),
                    decorations: text(5),
                    parents: text(6).split_whitespace().map(str::to_owned).collect(),
                    graph_prefix,
                    graph_before: std::mem::take(&mut graph),
                });
            } else {
                graph.extend(
                    String::from_utf8_lossy(field)
                        .split('\n')
                        .filter(|line| !line.is_empty())
                        .map(str::to_owned),
                );
            }
        }
    })
}

#[cfg(test)]
mod history_error_tests {
    use super::*;

    fn failed_output(code: i32, truncated: bool, stderr: &[u8]) -> Output {
        Output {
            stdout: Vec::new(),
            stderr: stderr.to_vec(),
            code,
            truncated,
        }
    }

    #[test]
    fn overflow_killed_git_reports_read_limit_before_its_empty_exit_error() {
        let output = failed_output(-1, true, b"");
        let error = history_output_error(&output, 1_000_001)
            .expect("overflow must fail rather than expose partial commits")
            .to_string();
        assert!(error.contains("512 MiB"), "{error}");
        assert!(error.contains("1000001"), "{error}");
        assert!(error.contains("Press L"), "{error}");
        assert!(error.contains("smaller"), "{error}");
        // Overflow remains the actionable cause even when Git did report stderr.
        let output = failed_output(-1, true, b"broken pipe");
        assert!(
            history_output_error(&output, 5_000)
                .unwrap()
                .to_string()
                .contains("512 MiB")
        );
    }

    #[test]
    fn history_failures_preserve_git_details_and_never_produce_an_empty_popup() {
        let output = failed_output(128, false, b"fatal: bad revision 'missing'\n");
        let error = history_output_error(&output, 300).unwrap().to_string();
        assert!(error.contains("300"), "{error}");
        assert!(error.contains("bad revision 'missing'"), "{error}");
        let output = failed_output(-1, false, b"");
        let error = history_output_error(&output, 1_000).unwrap().to_string();
        assert!(error.contains("1000"), "{error}");
        assert!(error.contains("exit code -1"), "{error}");
        assert!(history_output_error(&failed_output(0, false, b""), 1_000).is_none());
    }

    #[test]
    fn streamed_records_preserve_connector_rows_and_ignore_an_incomplete_final_record() {
        let first = "a".repeat(40);
        let second = "b".repeat(40);
        let bytes = format!(
            "* \0{first}\0aaaaaaa\0First subject\0Author One\02026-11-01\0HEAD -> main\0\0Committer One\02026-11-02\0\0|\\\n| * \0{second}\0bbbbbbb\0Second subject\0Author Two\02026-11-03\0feature\0{first}\0Committer Two\02026-11-04\0\0* \0{}\0partial",
            "c".repeat(40)
        );
        let commits = parse_commits(bytes.as_bytes());
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].oid, first);
        assert_eq!(commits[0].committer, "Committer One");
        assert_eq!(commits[1].oid, second);
        assert_eq!(commits[1].parents, vec![first]);
        assert_eq!(commits[1].graph_before, vec!["|\\"]);
        assert_eq!(commits[1].graph_prefix, "| * ");
        assert_eq!(commits[1].committed_at, "2026-11-04");
    }
}
