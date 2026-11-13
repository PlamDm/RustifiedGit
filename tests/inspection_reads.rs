use rustifiedgit::{
    config::GuiConfig, git::Repository, model::PreviewTarget, process::Cancellation,
};
use std::{fs, path::Path, process::Command};

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
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
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn fixture() -> (tempfile::TempDir, Repository) {
    let directory = tempfile::tempdir().unwrap();
    git(directory.path(), &["init", "-qb", "main"]);
    for (name, value) in [
        ("user.name", "Inspection Test"),
        ("user.email", "inspection@example.invalid"),
        ("commit.gpgsign", "false"),
        ("core.hooksPath", ".test-hooks"),
    ] {
        git(directory.path(), &["config", name, value]);
    }
    let repository = Repository::discover(directory.path()).unwrap();
    (directory, repository)
}

#[test]
fn combined_inspection_preserves_empty_commits_full_messages_and_rename_records() {
    let (directory, repository) = fixture();
    let root = directory.path();
    fs::write(root.join("old [name].txt"), "contents\n").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "root"]);
    let parent = git(root, &["rev-parse", "HEAD"]);
    git(root, &["mv", "old [name].txt", "new [name].txt"]);
    git(
        root,
        &[
            "commit",
            "-qm",
            "rename\n\nComplete message body.\n\nAnother paragraph.",
        ],
    );
    let oid = git(root, &["rev-parse", "HEAD"]);
    let inspection = repository
        .inspect_commit(&oid, &Cancellation::default())
        .unwrap();
    assert_eq!(inspection.details.parents, vec![parent]);
    assert_eq!(
        inspection.details.message,
        "rename\n\nComplete message body.\n\nAnother paragraph."
    );
    assert_eq!(
        inspection.files,
        repository
            .commit_files(&oid, &Cancellation::default())
            .unwrap()
    );
    assert_eq!(
        inspection.files[0].old_path.as_deref(),
        Some(Path::new("old [name].txt"))
    );
    git(root, &["commit", "--allow-empty", "-qm", "empty"]);
    let oid = git(root, &["rev-parse", "HEAD"]);
    let inspection = repository
        .inspect_commit(&oid, &Cancellation::default())
        .unwrap();
    assert_eq!(inspection.details.message, "empty");
    assert!(inspection.files.is_empty());
}

#[test]
fn selected_commit_preview_size_checks_use_literal_paths() {
    let (directory, repository) = fixture();
    let root = directory.path();
    fs::write(root.join("literal[1].txt"), "small base\n").unwrap();
    fs::write(root.join("literal1.txt"), "x".repeat(1000)).unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "base"]);
    fs::write(root.join("literal[1].txt"), "small change\n").unwrap();
    git(root, &["commit", "-qam", "change"]);
    let preview = repository
        .preview(
            &PreviewTarget::CommitFiles {
                oid: git(root, &["rev-parse", "HEAD"]),
                paths: vec!["literal[1].txt".into()],
            },
            false,
            &GuiConfig {
                diff_preview_max_file_bytes: 100,
                ..GuiConfig::default()
            },
            3,
            false,
            &Cancellation::default(),
        )
        .unwrap();
    assert!(!preview.suppressed, "{}", preview.text);
    assert!(preview.text.contains("+small change"));
}

#[test]
fn batched_selected_file_size_checks_cover_late_chunks_and_deleted_parent_blobs() {
    let (directory, repository) = fixture();
    let root = directory.path();
    let paths: Vec<_> = (0..80)
        .map(|index| format!("file-{index:03}-{}.txt", "a".repeat(120)))
        .collect();
    for path in &paths {
        fs::write(root.join(path), "small\n").unwrap();
    }
    fs::write(root.join(paths.last().unwrap()), "large\n".repeat(100)).unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "base"]);
    for path in &paths[..paths.len() - 1] {
        fs::write(root.join(path), "changed\n").unwrap();
    }
    fs::remove_file(root.join(paths.last().unwrap())).unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "change and delete"]);
    let target = PreviewTarget::CommitFiles {
        oid: git(root, &["rev-parse", "HEAD"]),
        paths: paths.into_iter().map(Into::into).collect(),
    };
    let settings = GuiConfig {
        diff_preview_max_file_bytes: 100,
        ..GuiConfig::default()
    };
    let automatic = repository
        .preview(
            &target,
            false,
            &settings,
            3,
            false,
            &Cancellation::default(),
        )
        .unwrap();
    assert!(automatic.suppressed);
    let explicit = repository
        .preview(&target, true, &settings, 3, false, &Cancellation::default())
        .unwrap();
    assert!(!explicit.suppressed);
    assert!(explicit.text.contains("deleted file mode"));
    assert!(explicit.text.contains("+changed"));
}
