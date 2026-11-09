use pd_lazygit::{
    config::GuiConfig,
    git::{ReadOptions, Repository},
    model::PreviewTarget,
    process::Cancellation,
    worker::{WorkerEvent, Workers},
};
use std::{fs, path::Path, process::Command, time::Duration};
use tempfile::TempDir;

struct Fixture {
    dir: TempDir,
    repo: Repository,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q", "-b", "main"]);
        git(dir.path(), &["config", "user.name", "Metadata Tests"]);
        git(
            dir.path(),
            &["config", "user.email", "metadata@example.invalid"],
        );
        git(dir.path(), &["config", "commit.gpgsign", "false"]);
        git(dir.path(), &["config", "core.hooksPath", ".test-hooks"]);
        let repo = Repository::discover(dir.path()).unwrap();
        Self { dir, repo }
    }

    fn commit(&self, message: &str) -> String {
        git(self.dir.path(), &["add", "--all"]);
        git(
            self.dir.path(),
            &["commit", "-qm", message, "--allow-empty"],
        );
        git(self.dir.path(), &["rev-parse", "HEAD"])
            .trim()
            .to_owned()
    }
}

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn tracking_counts_distinguish_sync_divergence_and_missing_upstreams() {
    let fixture = Fixture::new();
    let root = fixture.dir.path();
    let cancel = Cancellation::default();
    let initial = fixture.commit("initial");
    let snapshot = fixture
        .repo
        .status_snapshot(&ReadOptions::default(), &cancel)
        .unwrap();
    assert_eq!((snapshot.ahead, snapshot.behind), (None, None));

    git(root, &["config", "remote.origin.url", "."]);
    git(
        root,
        &[
            "config",
            "remote.origin.fetch",
            "+refs/heads/*:refs/remotes/origin/*",
        ],
    );
    git(root, &["update-ref", "refs/remotes/origin/main", &initial]);
    git(root, &["branch", "--set-upstream-to=origin/main", "main"]);
    let snapshot = fixture
        .repo
        .status_snapshot(&ReadOptions::default(), &cancel)
        .unwrap();
    assert_eq!((snapshot.ahead, snapshot.behind), (Some(0), Some(0)));
    assert_eq!(snapshot.upstream, "origin/main");

    git(root, &["checkout", "-qb", "remote-work"]);
    let upstream = fixture.commit("upstream only");
    git(root, &["update-ref", "refs/remotes/origin/main", &upstream]);
    git(root, &["checkout", "-q", "main"]);
    fixture.commit("local only");
    let snapshot = fixture
        .repo
        .status_snapshot(&ReadOptions::default(), &cancel)
        .unwrap();
    assert_eq!((snapshot.ahead, snapshot.behind), (Some(1), Some(1)));
    let current = snapshot
        .references
        .iter()
        .find(|reference| reference.current)
        .unwrap();
    assert_eq!((current.ahead, current.behind), (Some(1), Some(1)));
    let untracked = snapshot
        .references
        .iter()
        .find(|reference| reference.name == "remote-work")
        .unwrap();
    assert_eq!((untracked.ahead, untracked.behind), (None, None));

    git(root, &["update-ref", "-d", "refs/remotes/origin/main"]);
    let snapshot = fixture
        .repo
        .status_snapshot(&ReadOptions::default(), &cancel)
        .unwrap();
    assert_eq!((snapshot.ahead, snapshot.behind), (None, None));
    assert_eq!(snapshot.upstream, "origin/main");
    git(root, &["checkout", "--detach", "-q"]);
    let snapshot = fixture
        .repo
        .status_snapshot(&ReadOptions::default(), &cancel)
        .unwrap();
    assert_eq!((snapshot.ahead, snapshot.behind), (None, None));
    assert!(snapshot.upstream.is_empty());
}

#[test]
fn head_tree_and_contents_ignore_index_and_working_changes() {
    let fixture = Fixture::new();
    let root = fixture.dir.path();
    let cancel = Cancellation::default();
    assert!(fixture.repo.tree_files("", &cancel).unwrap().is_empty());
    fs::create_dir(root.join("nested")).unwrap();
    fs::write(
        root.join("nested/deleted.txt"),
        "committed deletion target\n",
    )
    .unwrap();
    let unusual = "nested/[literal]:file\nname.txt";
    fs::write(root.join(unusual), "committed contents\n").unwrap();
    let first = fixture.commit("initial tree");
    fs::remove_file(root.join("nested/deleted.txt")).unwrap();
    fs::write(root.join(unusual), "modified working contents\n").unwrap();
    fs::write(root.join("new.txt"), "index only\n").unwrap();
    git(root, &["add", "--all"]);

    let tree = fixture.repo.tree_files(&first, &cancel).unwrap();
    assert_eq!(tree.len(), 2);
    assert!(tree.iter().all(|file| !file.changed()));
    assert!(
        tree.iter()
            .any(|file| file.path == Path::new("nested/deleted.txt"))
    );
    assert!(tree.iter().any(|file| file.path == Path::new(unusual)));
    let preview = fixture
        .repo
        .preview(
            &PreviewTarget::TreeFile {
                path: unusual.into(),
                oid: first.clone(),
            },
            true,
            &GuiConfig::default(),
            3,
            false,
            &cancel,
        )
        .unwrap();
    assert_eq!(preview.text, "committed contents\n");
    assert!(preview.patch.is_empty());
    let second = fixture.commit("updated tree");
    let updated = fixture.repo.tree_files(&second, &cancel).unwrap();
    assert!(updated.iter().any(|file| file.path == Path::new("new.txt")));
    assert!(
        !updated
            .iter()
            .any(|file| file.path == Path::new("nested/deleted.txt"))
    );
    assert_eq!(fixture.repo.tree_files(&first, &cancel).unwrap(), tree);
    let cancelled = Cancellation::default();
    cancelled.cancel();
    assert!(fixture.repo.tree_files(&first, &cancelled).is_err());
}

#[test]
fn committed_blob_preview_honors_limits_and_never_exposes_a_patch() {
    let fixture = Fixture::new();
    let root = fixture.dir.path();
    let cancel = Cancellation::default();
    let contents = "diff --git a/file b/file\n--- a/file\n+++ b/file\n@@ -1 +1 @@\n-old\n+new\n";
    fs::write(root.join("patch.txt"), contents).unwrap();
    fs::write(root.join("binary.dat"), [1, 0, 2]).unwrap();
    let oid = fixture.commit("blobs");
    let target = PreviewTarget::TreeFile {
        path: "patch.txt".into(),
        oid: oid.clone(),
    };
    let mut settings = GuiConfig {
        diff_preview_max_file_bytes: 8,
        ..GuiConfig::default()
    };
    let preview = fixture
        .repo
        .preview(&target, false, &settings, 3, false, &cancel)
        .unwrap();
    assert!(preview.suppressed);
    assert!(preview.patch.is_empty());
    let preview = fixture
        .repo
        .preview(&target, true, &settings, 3, false, &cancel)
        .unwrap();
    assert_eq!(preview.text, contents);
    assert!(preview.patch.is_empty());
    settings.diff_preview_max_file_bytes = 0;
    settings.diff_preview_max_bytes = 8;
    assert!(
        fixture
            .repo
            .preview(&target, false, &settings, 3, false, &cancel)
            .unwrap()
            .suppressed
    );
    let preview = fixture
        .repo
        .preview(
            &PreviewTarget::TreeFile {
                path: "binary.dat".into(),
                oid,
            },
            true,
            &settings,
            3,
            false,
            &cancel,
        )
        .unwrap();
    assert_eq!(preview.text, "Binary file in HEAD.");
    assert!(preview.patch.is_empty());
}

#[test]
fn tree_worker_delivers_latest_immutable_head() {
    let fixture = Fixture::new();
    fs::write(fixture.dir.path().join("first.txt"), "first").unwrap();
    let first = fixture.commit("first");
    fs::write(fixture.dir.path().join("second.txt"), "second").unwrap();
    let second = fixture.commit("second");
    let workers = Workers::new(fixture.repo.clone());
    workers.head_tree.submit(first);
    let generation = workers.head_tree.submit(second.clone());
    loop {
        let event = workers.events.recv_timeout(Duration::from_secs(5)).unwrap();
        if let WorkerEvent::HeadTree(id, oid, result) = event {
            if id == generation {
                assert_eq!(oid, second);
                assert_eq!(result.unwrap().len(), 2);
                break;
            }
        }
    }
}
