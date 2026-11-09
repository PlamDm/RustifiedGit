use pd_lazygit::{
    config::GuiConfig,
    git::{Operation, ReadOptions, Repository, discard_operation, stage_operation},
    model::{FileEntry, PreviewTarget},
    patch::Patch,
    process::{Cancellation, CommandSpec},
};
use std::{
    ffi::OsStr,
    fs,
    path::Path,
    process::Command,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;

struct Fixture {
    dir: TempDir,
    repo: Repository,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), ["init", "-q", "-b", "main"]);
        git(dir.path(), ["config", "user.name", "Port Tests"]);
        git(
            dir.path(),
            ["config", "user.email", "tests@example.invalid"],
        );
        git(dir.path(), ["config", "commit.gpgsign", "false"]);
        git(dir.path(), ["config", "core.hooksPath", ".test-hooks"]);
        git(dir.path(), ["config", "core.autocrlf", "false"]);
        let repo = Repository::discover(dir.path()).unwrap();
        Self { dir, repo }
    }
    fn write(&self, path: impl AsRef<Path>, contents: impl AsRef<[u8]>) {
        let path = self.dir.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }
    fn commit(&self, message: &str) {
        git(self.dir.path(), ["add", "--all"]);
        git(self.dir.path(), ["commit", "-q", "-m", message]);
    }
    fn snapshot(&self) -> pd_lazygit::model::Snapshot {
        self.repo
            .snapshot(&ReadOptions::default(), &Cancellation::default())
            .unwrap()
    }
    fn execute(&self, operation: Operation) {
        self.repo
            .execute(&operation, &Cancellation::default(), None)
            .unwrap();
    }
    fn preview(
        &self,
        path: &str,
        staged: bool,
        explicit: bool,
        settings: &GuiConfig,
    ) -> pd_lazygit::model::Preview {
        self.repo
            .preview(
                &PreviewTarget::File {
                    path: path.into(),
                    old_path: None,
                    untracked: false,
                    ignored: false,
                    staged,
                    commit: None,
                },
                explicit,
                settings,
                3,
                false,
                &Cancellation::default(),
            )
            .unwrap()
    }
}

fn git(root: &Path, args: impl IntoIterator<Item = impl AsRef<OsStr>>) -> String {
    let result = Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "git failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8_lossy(&result.stdout).into_owned()
}

#[test]
fn clean_repository_keeps_all_tracked_files_and_honors_ignored_toggle() {
    let fixture = Fixture::new();
    fixture.write("nested/clean.txt", "clean\n");
    fixture.write(".gitignore", "*.log\n");
    fixture.commit("initial");
    fixture.write("private.log", "ignored");
    fixture.write("new file.txt", "untracked");
    let snapshot = fixture.snapshot();
    assert!(
        snapshot
            .files
            .iter()
            .any(|file| file.path == Path::new("nested/clean.txt") && !file.changed())
    );
    assert!(
        snapshot
            .files
            .iter()
            .any(|file| file.path == Path::new("new file.txt") && file.untracked())
    );
    assert!(
        !snapshot
            .files
            .iter()
            .any(|file| file.path == Path::new("private.log"))
    );
    let (files, _) = fixture.repo.files(true, &Cancellation::default()).unwrap();
    assert!(
        files
            .iter()
            .any(|file| file.path == Path::new("private.log") && file.ignored())
    );
    git(
        fixture.dir.path(),
        ["config", "status.showUntrackedFiles", "no"],
    );
    assert!(!fixture.snapshot().files.iter().any(|file| file.untracked()));
}

#[test]
fn stage_and_unstage_work_before_the_first_commit() {
    let fixture = Fixture::new();
    fixture.write("first.txt", "hello\n");
    let snapshot = fixture.snapshot();
    assert!(snapshot.head.is_empty());
    fixture.execute(
        stage_operation(&snapshot.files.iter().collect::<Vec<_>>(), false, false).unwrap(),
    );
    let snapshot = fixture.snapshot();
    assert!(snapshot.files[0].staged());
    fixture
        .execute(stage_operation(&snapshot.files.iter().collect::<Vec<_>>(), true, false).unwrap());
    assert!(fixture.snapshot().files[0].untracked());
    assert!(fixture.dir.path().join("first.txt").exists());
}

#[test]
fn stage_directory_excludes_ignored_and_unchanged_files() {
    let fixture = Fixture::new();
    fixture.write(".gitignore", "*.log\n");
    fixture.write("dir/clean", "base");
    fixture.commit("base");
    fixture.write("dir/new", "new");
    fixture.write("dir/secret.log", "secret");
    let (files, _) = fixture.repo.files(true, &Cancellation::default()).unwrap();
    let selected = pd_lazygit::tree::matching_files(&files, Path::new("dir"));
    fixture.execute(stage_operation(&selected, false, true).unwrap());
    assert_eq!(
        git(fixture.dir.path(), ["diff", "--cached", "--name-only"]).trim(),
        "dir/new"
    );
    assert!(fixture.dir.path().join("dir/secret.log").exists());
}

#[cfg(unix)]
#[test]
fn nul_status_and_native_paths_survive_renames_spaces_newlines_and_globs() {
    let fixture = Fixture::new();
    fixture.write("old file", "base\n");
    fixture.commit("base");
    git(
        fixture.dir.path(),
        ["mv", "old file", "new name\nwith tab\t"],
    );
    fixture.write(":(glob)*.txt", "literal\n");
    let snapshot = fixture.snapshot();
    let renamed = snapshot
        .files
        .iter()
        .find(|file| file.old_path.is_some())
        .unwrap();
    assert_eq!(renamed.path, Path::new("new name\nwith tab\t"));
    assert_eq!(renamed.old_path.as_deref(), Some(Path::new("old file")));
    fixture
        .execute(stage_operation(&snapshot.files.iter().collect::<Vec<_>>(), false, true).unwrap());
    assert!(
        git(
            fixture.dir.path(),
            ["diff", "--cached", "--name-only", "-z"]
        )
        .contains(":(glob)*.txt")
    );
}

#[test]
fn renames_with_spaces_stage_and_unstage_on_every_platform() {
    let fixture = Fixture::new();
    fixture.write("old file", "base\n");
    fixture.commit("base");
    git(fixture.dir.path(), ["mv", "old file", "new file"]);
    let snapshot = fixture.snapshot();
    fixture
        .execute(stage_operation(&snapshot.files.iter().collect::<Vec<_>>(), true, true).unwrap());
    assert!(
        fixture
            .snapshot()
            .files
            .iter()
            .any(|file| file.path == Path::new("old file") && file.worktree == b'D')
    );
    assert!(fixture.dir.path().join("new file").exists());
}

#[cfg(unix)]
#[test]
fn cancellation_also_handles_a_shell_that_exits_before_its_children() {
    let fixture = Fixture::new();
    let (sender, receiver) = mpsc::sync_channel(8);
    let token = Cancellation::default();
    let cancel = token.clone();
    let repo = fixture.repo.clone();
    let handle = thread::spawn(move || {
        repo.execute(
            &Operation::shell("sleep 30 & printf ready".into()),
            &token,
            Some(sender),
        )
    });
    assert!(
        String::from_utf8_lossy(&receiver.recv_timeout(Duration::from_secs(2)).unwrap())
            .contains("ready")
    );
    cancel.cancel();
    assert!(handle.join().unwrap().is_err());
}

#[test]
fn staged_deletion_checks_the_head_blob_before_previewing() {
    let fixture = Fixture::new();
    fixture.write("large", "x".repeat(500));
    fixture.commit("large");
    git(fixture.dir.path(), ["rm", "large"]);
    let settings = GuiConfig {
        diff_preview_max_file_bytes: 100,
        ..GuiConfig::default()
    };
    assert!(fixture.preview("large", true, false, &settings).suppressed);
    assert!(!fixture.preview("large", true, true, &settings).suppressed);
}

#[cfg(unix)]
#[test]
fn commits_keep_repository_hooks_and_external_diff_tool_configuration() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    fixture.write("file with spaces", "base\n");
    fixture.commit("base");
    fixture.write(
        ".test-hooks/pre-commit",
        "#!/bin/sh\nprintf 'hook-ran' > hook-result\n",
    );
    fs::set_permissions(
        fixture.dir.path().join(".test-hooks/pre-commit"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    fixture.write("file with spaces", "changed\n");
    fixture.execute(Operation::git(["add", "--", "file with spaces"]));
    fixture.execute(Operation::git(["commit", "-m", "through-rust"]));
    assert_eq!(
        fs::read_to_string(fixture.dir.path().join("hook-result")).unwrap(),
        "hook-ran"
    );
    git(fixture.dir.path(), ["config", "diff.tool", "port-test"]);
    git(
        fixture.dir.path(),
        [
            "config",
            "difftool.port-test.cmd",
            "printf '%s' \"$MERGED\" > tool-result",
        ],
    );
    fixture.write("file with spaces", "next\n");
    fixture.execute(Operation::git([
        "difftool",
        "--no-prompt",
        "--",
        "file with spaces",
    ]));
    assert_eq!(
        fs::read_to_string(fixture.dir.path().join("tool-result")).unwrap(),
        "file with spaces"
    );
}

#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn non_utf8_filenames_are_not_replaced_when_staging() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    let fixture = Fixture::new();
    let name = OsString::from_vec(b"raw-\xff.txt".to_vec());
    fixture.write(Path::new(&name), "bytes");
    let snapshot = fixture.snapshot();
    assert_eq!(snapshot.files[0].path.as_os_str(), &name);
    fixture.execute(
        stage_operation(&snapshot.files.iter().collect::<Vec<_>>(), false, false).unwrap(),
    );
    assert!(fixture.snapshot().files[0].staged());
}

#[test]
fn staged_and_unstaged_versions_have_separate_previews() {
    let fixture = Fixture::new();
    fixture.write("file", "base\n");
    fixture.commit("base");
    fixture.write("file", "staged\n");
    git(fixture.dir.path(), ["add", "file"]);
    fixture.write("file", "working\n");
    let unstaged = fixture.preview("file", false, false, &GuiConfig::default());
    let staged = fixture.preview("file", true, false, &GuiConfig::default());
    assert!(unstaged.text.contains("+working"));
    assert!(staged.text.contains("+staged"));
    assert!(!staged.text.contains("+working"));
}

#[test]
fn automatic_preview_limits_check_worktree_and_index_and_explicit_bypasses_them() {
    let fixture = Fixture::new();
    fixture.write("file", "small\n");
    fixture.commit("base");
    fixture.write("file", "x".repeat(500));
    let settings = GuiConfig {
        diff_preview_max_file_bytes: 100,
        diff_preview_max_bytes: 1000,
        ..GuiConfig::default()
    };
    assert!(fixture.preview("file", false, false, &settings).suppressed);
    assert!(!fixture.preview("file", false, true, &settings).suppressed);
    git(fixture.dir.path(), ["add", "file"]);
    fixture.write("file", "tiny\n");
    assert!(fixture.preview("file", false, false, &settings).suppressed);
    let settings = GuiConfig {
        diff_preview_max_file_bytes: 0,
        diff_preview_max_bytes: 50,
        ..GuiConfig::default()
    };
    assert!(fixture.preview("file", false, false, &settings).suppressed);
    assert!(!fixture.preview("file", false, true, &settings).suppressed);
}

#[test]
fn committed_file_previews_check_both_sides_of_a_change() {
    let fixture = Fixture::new();
    fixture.write("file", "x".repeat(500));
    fixture.commit("large");
    fixture.write("file", "small\n");
    fixture.commit("small");
    let target = PreviewTarget::File {
        path: "file".into(),
        old_path: None,
        untracked: false,
        ignored: false,
        staged: false,
        commit: Some(fixture.snapshot().head),
    };
    let settings = GuiConfig {
        diff_preview_max_file_bytes: 100,
        ..GuiConfig::default()
    };
    assert!(
        fixture
            .repo
            .preview(
                &target,
                false,
                &settings,
                3,
                false,
                &Cancellation::default()
            )
            .unwrap()
            .suppressed
    );
    assert!(
        !fixture
            .repo
            .preview(&target, true, &settings, 3, false, &Cancellation::default())
            .unwrap()
            .suppressed
    );
}

#[test]
fn individual_hunks_can_be_staged_and_unstaged_without_other_changes() {
    let fixture = Fixture::new();
    let base: String = (0..30).map(|i| format!("line {i}\n")).collect();
    fixture.write("file", &base);
    fixture.commit("base");
    fixture.write(
        "file",
        base.replace("line 2\n", "edited 2\n")
            .replace("line 25\n", "edited 25\n"),
    );
    let patch = Patch::parse(
        &fixture
            .preview("file", false, true, &GuiConfig::default())
            .patch,
    );
    assert_eq!(patch.hunks.len(), 2);
    fixture.execute(Operation::patch(patch.hunk_patch(1).unwrap(), true, false));
    let staged = fixture.preview("file", true, true, &GuiConfig::default());
    assert!(staged.text.contains("+edited 25"));
    assert!(!staged.text.contains("+edited 2\n"));
    let patch = Patch::parse(&staged.patch);
    fixture.execute(Operation::patch(patch.hunk_patch(0).unwrap(), true, true));
    assert!(git(fixture.dir.path(), ["diff", "--cached"]).is_empty());
    assert!(
        fixture
            .preview("file", false, true, &GuiConfig::default())
            .text
            .contains("+edited 25")
    );
}

#[test]
fn individual_added_and_removed_lines_can_be_staged_and_unstaged() {
    let fixture = Fixture::new();
    fixture.write("file", "one\ntwo\nthree\nfour\n");
    fixture.commit("base");
    fixture.write("file", "one\nTWO\nTHREE\nfour\n");
    let patch = Patch::parse(
        &fixture
            .preview("file", false, true, &GuiConfig::default())
            .patch,
    );
    let line = patch
        .lines
        .iter()
        .position(|line| line == b"+TWO\n")
        .unwrap();
    fixture.execute(Operation::patch(
        patch.line_patch(line, false).unwrap(),
        true,
        false,
    ));
    let staged = fixture.preview("file", true, true, &GuiConfig::default());
    assert!(staged.text.contains("+TWO"));
    assert!(!staged.text.contains("+THREE"));
    let patch = Patch::parse(&staged.patch);
    let line = patch
        .lines
        .iter()
        .position(|line| line == b"+TWO\n")
        .unwrap();
    fixture.execute(Operation::patch(
        patch.line_patch(line, true).unwrap(),
        true,
        true,
    ));
    assert!(git(fixture.dir.path(), ["diff", "--cached"]).is_empty());
    let patch = Patch::parse(
        &fixture
            .preview("file", false, true, &GuiConfig::default())
            .patch,
    );
    let line = patch
        .lines
        .iter()
        .position(|line| line == b"-two\n")
        .unwrap();
    fixture.execute(Operation::patch(
        patch.line_patch(line, false).unwrap(),
        true,
        false,
    ));
    assert_eq!(
        git(fixture.dir.path(), ["show", ":file"]),
        "one\nthree\nfour\n"
    );
}

#[test]
fn history_follows_renames_and_commit_files_report_them() {
    let fixture = Fixture::new();
    fixture.write("before", "content\n");
    fixture.commit("before");
    git(fixture.dir.path(), ["mv", "before", "after"]);
    fixture.commit("renamed");
    let options = ReadOptions {
        history_path: Some("after".into()),
        ..ReadOptions::default()
    };
    let commits = fixture
        .repo
        .commits(&options, &Cancellation::default())
        .unwrap();
    assert_eq!(commits.len(), 2);
    assert_eq!(commits[1].subject, "before");
    let files = fixture
        .repo
        .commit_files(&commits[0].oid, &Cancellation::default())
        .unwrap();
    assert_eq!(files[0].path, Path::new("after"));
    assert_eq!(files[0].old_path.as_deref(), Some(Path::new("before")));
}

#[test]
fn branch_stash_worktree_and_commit_operations_round_trip() {
    let fixture = Fixture::new();
    fixture.write("file", "base\n");
    fixture.commit("base");
    fixture.execute(Operation::git(["switch", "-c", "feature"]));
    fixture.write("file", "change\n");
    fixture.execute(Operation::git(["stash", "push", "-m", "saved"]));
    let snapshot = fixture.snapshot();
    assert_eq!(snapshot.stashes.len(), 1);
    assert!(
        snapshot
            .references
            .iter()
            .any(|reference| reference.name == "feature" && reference.current)
    );
    fixture.execute(Operation::git(["stash", "pop"]));
    assert!(fixture.snapshot().stashes.is_empty());
    let snapshot = fixture.snapshot();
    fixture
        .execute(stage_operation(&snapshot.files.iter().collect::<Vec<_>>(), false, true).unwrap());
    fixture.execute(Operation::git(["commit", "-m", "feature change"]));
    assert_eq!(fixture.snapshot().commits[0].subject, "feature change");
    let other = fixture.dir.path().join("other-worktree");
    git(
        fixture.dir.path(),
        [
            OsStr::new("worktree"),
            OsStr::new("add"),
            OsStr::new("-q"),
            other.as_os_str(),
            OsStr::new("main"),
        ],
    );
    let other_repo = Repository::discover(&other).unwrap();
    assert_ne!(other_repo.git_dir, fixture.repo.git_dir);
    assert_eq!(fixture.snapshot().worktrees.len(), 2);
}

#[test]
fn discard_restores_tracked_changes_without_deleting_ignored_files() {
    let fixture = Fixture::new();
    fixture.write("file", "base\n");
    fixture.write(".gitignore", "*.log\n");
    fixture.commit("base");
    fixture.write("file", "modified\n");
    fixture.write("new", "delete");
    fixture.write("private.log", "keep");
    let (files, _) = fixture.repo.files(true, &Cancellation::default()).unwrap();
    fixture.execute(discard_operation(&files.iter().collect::<Vec<_>>(), true).unwrap());
    assert_eq!(
        fs::read_to_string(fixture.dir.path().join("file")).unwrap(),
        "base\n"
    );
    assert!(!fixture.dir.path().join("new").exists());
    assert!(fixture.dir.path().join("private.log").exists());
}

#[cfg(unix)]
#[test]
fn command_output_streams_before_completion_and_cancellation_stops_process_group() {
    let fixture = Fixture::new();
    let (output, receiver) = mpsc::sync_channel(16);
    let cancel = Cancellation::default();
    let token = cancel.clone();
    let repo = fixture.repo.clone();
    let started = Instant::now();
    let handle = thread::spawn(move || {
        repo.execute(
            &Operation::shell("printf 'ready\\n'; sleep 30; printf late".into()),
            &token,
            Some(output),
        )
    });
    let chunk = receiver.recv_timeout(Duration::from_secs(3)).unwrap();
    assert!(String::from_utf8_lossy(&chunk).contains("ready"));
    cancel.cancel();
    assert!(handle.join().unwrap().is_err());
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn read_only_snapshots_do_not_create_index_locks() {
    let fixture = Fixture::new();
    fixture.write("file", "base");
    fixture.commit("base");
    let before = fs::read(fixture.repo.git_dir.join("index")).unwrap();
    for _ in 0..3 {
        fixture.snapshot();
    }
    assert_eq!(
        before,
        fs::read(fixture.repo.git_dir.join("index")).unwrap()
    );
    assert!(!fixture.repo.git_dir.join("index.lock").exists());
}

#[test]
fn index_size_checks_cover_every_conflict_stage() {
    let fixture = Fixture::new();
    fixture.write("file", "base\n");
    fixture.commit("base");
    git(fixture.dir.path(), ["checkout", "-q", "-b", "other"]);
    fixture.write("file", "x".repeat(500));
    fixture.commit("other");
    git(fixture.dir.path(), ["checkout", "-q", "main"]);
    fixture.write("file", "main\n");
    fixture.commit("main");
    let status = Command::new("git")
        .args(["merge", "other"])
        .current_dir(fixture.dir.path())
        .output()
        .unwrap();
    assert!(!status.status.success());
    fixture.write("file", "small resolution\n");
    let settings = GuiConfig {
        diff_preview_max_file_bytes: 100,
        ..GuiConfig::default()
    };
    assert!(fixture.preview("file", false, false, &settings).suppressed);
}

#[test]
fn untracked_preview_uses_git_diff_without_adding_the_file_to_the_index() {
    let fixture = Fixture::new();
    fixture.write("new", "new contents\n");
    let preview = fixture
        .repo
        .preview(
            &PreviewTarget::File {
                path: "new".into(),
                old_path: None,
                untracked: true,
                ignored: false,
                staged: false,
                commit: None,
            },
            false,
            &GuiConfig::default(),
            3,
            false,
            &Cancellation::default(),
        )
        .unwrap();
    assert!(preview.text.contains("+new contents"));
    assert!(fixture.snapshot().files[0].untracked());
}

#[test]
fn error_messages_preserve_git_failures() {
    let fixture = Fixture::new();
    let error = fixture
        .repo
        .execute(
            &Operation::git(["switch", "missing-branch"]),
            &Cancellation::default(),
            None,
        )
        .unwrap_err()
        .to_string();
    assert!(error.contains("missing-branch"));
    let malformed = FileEntry {
        path: "../outside".into(),
        old_path: None,
        index: b'?',
        worktree: b'?',
    };
    assert!(stage_operation(&[&malformed], false, false).is_err());
    assert!(
        pd_lazygit::process::run(
            fixture.dir.path(),
            &CommandSpec::git(["--version"]),
            &Cancellation::default(),
            0,
            None,
            Duration::from_secs(2)
        )
        .unwrap()
        .truncated
    );
}

#[test]
fn workspace_nuke_stashes_submodules_and_resets_their_recorded_commit() {
    let source = Fixture::new();
    source.write("module-file", "recorded\n");
    source.commit("module base");
    let fixture = Fixture::new();
    fixture.write("root-file", "base\n");
    fixture.write(".gitignore", "*.log\n");
    fixture.commit("root base");
    git(
        fixture.dir.path(),
        [
            OsStr::new("-c"),
            OsStr::new("protocol.file.allow=always"),
            OsStr::new("submodule"),
            OsStr::new("add"),
            OsStr::new("--"),
            source.dir.path().as_os_str(),
            OsStr::new("module"),
        ],
    );
    fixture.commit("record submodule");
    let recorded = git(fixture.dir.path(), ["-C", "module", "rev-parse", "HEAD"]);
    git(
        fixture.dir.path(),
        ["-C", "module", "config", "user.name", "Port Tests"],
    );
    git(
        fixture.dir.path(),
        [
            "-C",
            "module",
            "config",
            "user.email",
            "tests@example.invalid",
        ],
    );
    git(
        fixture.dir.path(),
        ["-C", "module", "config", "commit.gpgsign", "false"],
    );
    git(
        fixture.dir.path(),
        [
            "-C",
            "module",
            "commit",
            "--allow-empty",
            "-m",
            "new module commit",
        ],
    );
    fixture.write("module/module-file", "dirty\n");
    fixture.write("module/untracked", "save me\n");
    fixture.write("root-file", "dirty root\n");
    fixture.write("untracked-root", "remove me");
    fixture.write("private.log", "keep ignored");
    fixture.execute(pd_lazygit::git::nuke_working_tree_operation());
    assert_eq!(
        git(fixture.dir.path(), ["-C", "module", "rev-parse", "HEAD"]),
        recorded
    );
    assert_eq!(
        git(fixture.dir.path(), ["-C", "module", "stash", "list"])
            .lines()
            .count(),
        1
    );
    assert_eq!(
        fs::read_to_string(fixture.dir.path().join("root-file")).unwrap(),
        "base\n"
    );
    assert!(!fixture.dir.path().join("untracked-root").exists());
    assert!(fixture.dir.path().join("private.log").exists());
    let snapshot = fixture.snapshot();
    assert!(
        snapshot.files.iter().all(|file| !file.changed()),
        "Files after nuke: {:?}; submodule status: {}; root diff: {}",
        snapshot.files,
        git(
            fixture.dir.path(),
            ["-C", "module", "status", "--porcelain"]
        ),
        git(fixture.dir.path(), ["diff", "--submodule"])
    );
}

#[test]
fn local_remote_push_and_fetch_keep_git_configuration() {
    let fixture = Fixture::new();
    fixture.write("file", "base\n");
    fixture.commit("base");
    let remote = tempfile::tempdir().unwrap();
    git(remote.path(), ["init", "--bare", "-q"]);
    git(
        fixture.dir.path(),
        [
            OsStr::new("remote"),
            OsStr::new("add"),
            OsStr::new("origin"),
            remote.path().as_os_str(),
        ],
    );
    fixture.execute(Operation::git(["push", "--set-upstream", "origin", "main"]));
    fixture.execute(Operation::git(["fetch", "--all", "--prune"]));
    let snapshot = fixture.snapshot();
    assert_eq!(snapshot.upstream, "origin/main");
    assert!(
        snapshot
            .references
            .iter()
            .any(|reference| reference.name == "origin/main")
    );
}

#[test]
fn merge_and_root_commits_keep_their_file_lists_and_parent_metadata() {
    let fixture = Fixture::new();
    fixture.write("base", "base\n");
    fixture.commit("root");
    assert!(fixture.snapshot().commits[0].parents.is_empty());
    let files = fixture
        .repo
        .commit_files(&fixture.snapshot().head, &Cancellation::default())
        .unwrap();
    assert_eq!(files[0].path, Path::new("base"));
    git(fixture.dir.path(), ["switch", "-q", "-c", "feature"]);
    fixture.write("feature-file", "feature\n");
    fixture.commit("feature");
    git(fixture.dir.path(), ["switch", "-q", "main"]);
    fixture.write("main-file", "main\n");
    fixture.commit("main");
    git(
        fixture.dir.path(),
        ["merge", "--no-ff", "feature", "-m", "merge"],
    );
    let snapshot = fixture.snapshot();
    assert_eq!(snapshot.commits[0].parents.len(), 2);
    let files = fixture
        .repo
        .commit_files(&snapshot.head, &Cancellation::default())
        .unwrap();
    assert!(
        files
            .iter()
            .any(|file| file.path == Path::new("feature-file"))
    );
    let target = PreviewTarget::File {
        path: "feature-file".into(),
        old_path: None,
        untracked: false,
        ignored: false,
        staged: false,
        commit: Some(snapshot.head),
    };
    assert!(
        fixture
            .repo
            .preview(
                &target,
                true,
                &GuiConfig::default(),
                3,
                false,
                &Cancellation::default()
            )
            .unwrap()
            .text
            .contains("+feature")
    );
}

#[test]
fn partial_line_actions_work_for_new_and_deleted_files() {
    let fixture = Fixture::new();
    fixture.write("new", "alpha\nbeta\n");
    let target = PreviewTarget::File {
        path: "new".into(),
        old_path: None,
        untracked: true,
        ignored: false,
        staged: false,
        commit: None,
    };
    let preview = fixture
        .repo
        .preview(
            &target,
            true,
            &GuiConfig::default(),
            3,
            false,
            &Cancellation::default(),
        )
        .unwrap();
    let patch = Patch::parse(&preview.patch);
    let selected = patch
        .lines
        .iter()
        .position(|line| line == b"+alpha\n")
        .unwrap();
    fixture.execute(Operation::patch(
        patch.line_patch(selected, false).unwrap(),
        true,
        false,
    ));
    assert_eq!(git(fixture.dir.path(), ["show", ":new"]), "alpha\n");
    git(fixture.dir.path(), ["add", "new"]);
    let patch = Patch::parse(
        &fixture
            .preview("new", true, true, &GuiConfig::default())
            .patch,
    );
    let selected = patch
        .lines
        .iter()
        .position(|line| line == b"+alpha\n")
        .unwrap();
    fixture.execute(Operation::patch(
        patch.line_patch(selected, true).unwrap(),
        true,
        true,
    ));
    assert_eq!(git(fixture.dir.path(), ["show", ":new"]), "beta\n");
    fixture.commit("new file");
    fs::remove_file(fixture.dir.path().join("new")).unwrap();
    let patch = Patch::parse(
        &fixture
            .preview("new", false, true, &GuiConfig::default())
            .patch,
    );
    let selected = patch
        .lines
        .iter()
        .position(|line| line == b"-alpha\n")
        .unwrap();
    fixture.execute(Operation::patch(
        patch.line_patch(selected, false).unwrap(),
        true,
        false,
    ));
    assert_eq!(git(fixture.dir.path(), ["show", ":new"]), "beta\n");
    git(fixture.dir.path(), ["rm", "new"]);
    let patch = Patch::parse(
        &fixture
            .preview("new", true, true, &GuiConfig::default())
            .patch,
    );
    let selected = patch
        .lines
        .iter()
        .position(|line| line == b"-alpha\n")
        .unwrap();
    fixture.execute(Operation::patch(
        patch.line_patch(selected, true).unwrap(),
        true,
        true,
    ));
    assert_eq!(git(fixture.dir.path(), ["show", ":new"]), "alpha\n");
}

#[test]
fn headless_cli_renders_loaded_state_and_filters_history() {
    let fixture = Fixture::new();
    fixture.write("one", "one\n");
    fixture.commit("one");
    fixture.write("two", "two\n");
    fixture.commit("two");
    let binary = env!("CARGO_BIN_EXE_pd-lazygit");
    let output = Command::new(binary)
        .args([
            OsStr::new("-p"),
            fixture.dir.path().as_os_str(),
            OsStr::new("--snapshot"),
            OsStr::new("--filter"),
            OsStr::new("one"),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let snapshot: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(snapshot["commits"][0]["subject"], "one");
    let config = tempfile::tempdir().unwrap();
    let output = Command::new(binary)
        .args([
            OsStr::new("-p"),
            fixture.dir.path().as_os_str(),
            OsStr::new("--use-config-dir"),
            config.path().as_os_str(),
            OsStr::new("--render"),
            OsStr::new("100x30"),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let screen = String::from_utf8_lossy(&output.stdout);
    assert!(screen.contains("Repository status"));
    assert!(!screen.contains("Loading repository"));
}

#[test]
fn graph_queries_show_the_union_of_enabled_branches_without_checking_them_out() {
    let fixture = Fixture::new();
    fixture.write("base", "base");
    fixture.commit("base");
    git(fixture.dir.path(), ["switch", "-q", "-c", "left"]);
    fixture.write("left", "left");
    fixture.commit("left change");
    git(fixture.dir.path(), ["switch", "-q", "main"]);
    fixture.write("right", "right");
    fixture.commit("right change");
    let head = git(fixture.dir.path(), ["rev-parse", "HEAD"]);
    let mut options = ReadOptions {
        graph: true,
        visible_refs: Some(vec!["refs/heads/left".into()]),
        ..ReadOptions::default()
    };
    let left = fixture
        .repo
        .commits(&options, &Cancellation::default())
        .unwrap();
    assert!(left.iter().any(|commit| commit.subject == "left change"));
    assert!(!left.iter().any(|commit| commit.subject == "right change"));
    options
        .visible_refs
        .as_mut()
        .unwrap()
        .push("refs/heads/main".into());
    let both = fixture
        .repo
        .commits(&options, &Cancellation::default())
        .unwrap();
    assert_eq!(both.len(), 3);
    assert!(
        both.iter()
            .all(|commit| commit.oid.bytes().all(|byte| byte.is_ascii_hexdigit()))
    );
    assert!(both.iter().all(|commit| commit.graph_prefix.contains('*')));
    for (index, commit) in both.iter().enumerate() {
        for parent in &commit.parents {
            assert!(
                both.iter()
                    .position(|other| &other.oid == parent)
                    .is_some_and(|parent_index| parent_index > index)
            );
        }
    }
    assert_eq!(git(fixture.dir.path(), ["rev-parse", "HEAD"]), head);
    options.visible_refs = Some(Vec::new());
    assert!(
        fixture
            .repo
            .commits(&options, &Cancellation::default())
            .unwrap()
            .is_empty()
    );
    options.visible_refs = Some(vec!["refs/heads/deleted".into()]);
    assert!(
        fixture
            .repo
            .commits(&options, &Cancellation::default())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn graph_parser_preserves_merge_connectors_and_commit_message_fields() {
    let fixture = Fixture::new();
    fixture.write("base", "base");
    fixture.commit("base");
    git(fixture.dir.path(), ["switch", "-q", "-c", "left"]);
    fixture.write("left", "left");
    fixture.commit("left");
    git(fixture.dir.path(), ["switch", "-q", "main"]);
    fixture.write("right", "right");
    fixture.commit("right");
    git(
        fixture.dir.path(),
        ["merge", "--no-ff", "left", "-m", "merge"],
    );
    let options = ReadOptions {
        graph: true,
        visible_refs: Some(vec!["refs/heads/main".into()]),
        ..ReadOptions::default()
    };
    let commits = fixture
        .repo
        .commits(&options, &Cancellation::default())
        .unwrap();
    assert_eq!(commits.len(), 4);
    assert_eq!(commits[0].parents.len(), 2);
    assert_eq!(commits[0].subject, "merge");
    assert!(
        commits
            .iter()
            .flat_map(|commit| &commit.graph_before)
            .any(|line| line.contains('\\') || line.contains('/'))
    );
    assert!(commits.iter().all(|commit| commit.author == "Port Tests"));
    let inspection = fixture
        .repo
        .inspect_commit(&commits[0].oid, &Cancellation::default())
        .unwrap();
    assert_eq!(inspection.details.message, "merge");
    assert_eq!(inspection.details.author_email, "tests@example.invalid");
    assert_eq!(inspection.details.parents.len(), 2);
    assert!(
        inspection
            .files
            .iter()
            .any(|file| file.path == Path::new("left"))
    );
}

#[test]
fn commit_inspection_keeps_the_complete_message_body() {
    let fixture = Fixture::new();
    fixture.write("file", "file");
    fixture.commit("Subject\n\nDetailed explanation.\nAnother paragraph.");
    let oid = fixture.snapshot().head;
    let inspection = fixture
        .repo
        .inspect_commit(&oid, &Cancellation::default())
        .unwrap();
    assert_eq!(
        inspection.details.message,
        "Subject\n\nDetailed explanation.\nAnother paragraph."
    );
    assert_eq!(inspection.details.oid, oid);
    assert_eq!(inspection.files.len(), 1);
}
