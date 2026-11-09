use serde::Serialize;
use std::path::PathBuf;

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
    pub decorations: String,
    pub parents: Vec<String>,
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
    pub commits: Vec<Commit>,
    pub stashes: Vec<Stash>,
    pub worktrees: Vec<Worktree>,
    pub branch: String,
    pub head: String,
    pub upstream: String,
    pub operation: String,
    pub elapsed_ms: u128,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PreviewTarget {
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
}

impl Panel {
    pub fn number(self) -> usize {
        match self {
            Self::Status => 1,
            Self::Files => 2,
            Self::Branches => 3,
            Self::Commits => 4,
            Self::Stash => 5,
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
        }
    }
}
