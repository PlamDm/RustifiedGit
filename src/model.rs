use serde::Serialize;
use std::{path::PathBuf, sync::Arc};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FileEntry {
    pub path: PathBuf,
    pub old_path: Option<PathBuf>,
    pub index: u8,
    pub worktree: u8,
}

impl FileEntry {
    pub fn clean(path: PathBuf) -> Self {
        Self {
            path,
            old_path: None,
            index: b' ',
            worktree: b' ',
        }
    }
    pub fn ignored(&self) -> bool {
        self.index == b'!'
    }
    pub fn untracked(&self) -> bool {
        self.index == b'?'
    }
    pub fn changed(&self) -> bool {
        !self.ignored() && (self.index != b' ' || self.worktree != b' ')
    }
    pub fn staged(&self) -> bool {
        self.changed() && !self.untracked() && self.index != b' '
    }
    pub fn unstaged(&self) -> bool {
        self.changed() && self.worktree != b' '
    }
    pub fn conflicted(&self) -> bool {
        self.index == b'U'
            || self.worktree == b'U'
            || matches!((self.index, self.worktree), (b'A', b'A') | (b'D', b'D'))
    }
    pub fn status(&self) -> String {
        String::from_utf8_lossy(&[self.index, self.worktree]).into_owned()
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Reference {
    pub name: String,
    pub full_name: String,
    pub oid: String,
    pub upstream: String,
    pub ahead: Option<usize>,
    pub behind: Option<usize>,
    pub current: bool,
    pub kind: RefKind,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub enum RefKind {
    #[default]
    Branch,
    Remote,
    Tag,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Commit {
    pub oid: String,
    pub short: String,
    pub subject: String,
    pub author: String,
    pub date: String,
    pub committer: String,
    pub committed_at: String,
    pub decorations: String,
    pub parents: Vec<String>,
    pub graph_prefix: String,
    pub graph_before: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct CommitDetails {
    pub oid: String,
    pub message: String,
    pub author: String,
    pub author_email: String,
    pub committer: String,
    pub committer_email: String,
    pub committed_at: String,
    pub parents: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct CommitInspection {
    pub details: CommitDetails,
    pub files: Vec<FileEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Revision {
    pub name: String,
    pub revision: String,
}

impl Revision {
    pub fn named(name: String) -> Self {
        Self {
            revision: name.clone(),
            name,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComparisonRequest {
    pub from: Revision,
    pub to: Revision,
    pub merge_base: bool,
}

#[derive(Clone, Debug)]
pub struct ComparisonInfo {
    pub request: ComparisonRequest,
    pub from_oid: String,
    pub to_oid: String,
    pub merge_base_oid: Option<String>,
    pub summary: String,
}

impl ComparisonInfo {
    pub fn diff_base(&self) -> &str {
        self.merge_base_oid.as_deref().unwrap_or(&self.from_oid)
    }
    pub fn title(&self) -> String {
        if self.request.merge_base {
            format!(
                "{} since base {}",
                self.request.to.name, self.request.from.name
            )
        } else {
            format!("{} → {}", self.request.from.name, self.request.to.name)
        }
    }
}

#[derive(Clone, Debug)]
pub struct ComparisonResult {
    pub info: ComparisonInfo,
    pub files: Vec<FileEntry>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Stash {
    pub reference: String,
    pub oid: String,
    pub subject: String,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: String,
    pub oid: String,
    pub locked: bool,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Snapshot {
    pub files: Vec<FileEntry>,
    pub references: Vec<Reference>,
    pub commits: Arc<Vec<Commit>>,
    pub stashes: Vec<Stash>,
    pub worktrees: Vec<Worktree>,
    pub branch: String,
    pub head: String,
    pub upstream: String,
    pub ahead: Option<usize>,
    pub behind: Option<usize>,
    pub operation: String,
    pub elapsed_ms: u128,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PreviewTarget {
    CommitFiles {
        oid: String,
        paths: Vec<PathBuf>,
    },
    Comparison {
        from: String,
        to: String,
        path: PathBuf,
        old_path: Option<PathBuf>,
    },
    TreeFile {
        path: PathBuf,
        oid: String,
    },
    File {
        path: PathBuf,
        old_path: Option<PathBuf>,
        untracked: bool,
        ignored: bool,
        staged: bool,
        commit: Option<String>,
    },
    Commit(String),
    Reference(String),
    Stash(String),
    Status,
}

#[derive(Clone, Debug, Default)]
pub struct Preview {
    pub title: String,
    pub text: String,
    pub patch: Vec<u8>,
    pub suppressed: bool,
    pub staged: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Panel {
    Status,
    #[default]
    Files,
    Branches,
    Commits,
    Stash,
    Output,
    Diff,
    CommitFiles,
    HeadFiles,
    RemoteBranches,
    Details,
}

impl Panel {
    pub fn title(self) -> &'static str {
        match self {
            Self::Status => "Repositories",
            Self::Files => "Working files",
            Self::Branches => "Local branches",
            Self::RemoteBranches => "Remote branches",
            Self::Commits => "Commit graph",
            Self::Stash => "Stashes",
            Self::Details => "Commit details",
            Self::Output => "Command output",
            Self::Diff => "Diff",
            Self::CommitFiles => "Committed files",
            Self::HeadFiles => "HEAD tree",
        }
    }
    pub fn number(self) -> usize {
        match self {
            Self::Status => 1,
            Self::Files | Self::CommitFiles | Self::HeadFiles => 7,
            Self::Branches => 2,
            Self::RemoteBranches => 3,
            Self::Commits => 5,
            Self::Stash => 4,
            Self::Details => 6,
            Self::Output => 6,
            _ => 0,
        }
    }
    pub fn context(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Files => "files",
            Self::Branches => "localBranches",
            Self::Commits => "commits",
            Self::Stash => "stash",
            Self::Output => "commandOutput",
            Self::Diff => "main",
            Self::CommitFiles => "commitFiles",
            Self::HeadFiles => "headFiles",
            Self::RemoteBranches => "remoteBranches",
            Self::Details => "commitDetails",
        }
    }
}
