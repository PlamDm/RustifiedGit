use crate::{
    config::GuiConfig,
    git::{Operation, ReadOptions, Repository},
    model::{Commit, CommitInspection, FileEntry, Preview, PreviewTarget, Reference, Snapshot},
    process::Cancellation,
};
use std::{
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender, SyncSender},
    },
    thread,
    time::Instant,
};

type Pending<T> = Arc<(Mutex<(Option<(u64, T)>, bool)>, Condvar)>;

pub struct Latest<T> {
    pending: Pending<T>,
    generation: Arc<AtomicU64>,
}

impl<T: Send + 'static> Latest<T> {
    pub fn spawn(mut handle: impl FnMut(u64, T, Cancellation) + Send + 'static) -> Self {
        let pending: Pending<T> = Arc::new((Mutex::new((None, false)), Condvar::new()));
        let generation = Arc::new(AtomicU64::new(0));
        let state = pending.clone();
        let requested = generation.clone();
        thread::spawn(move || {
            loop {
                let (lock, notify) = &*state;
                let mut guard = lock.lock().unwrap();
                while guard.0.is_none() && !guard.1 {
                    guard = notify.wait(guard).unwrap();
                }
                if guard.1 {
                    break;
                }
                let (id, request) = guard.0.take().unwrap();
                drop(guard);
                handle(id, request, Cancellation::generation(requested.clone(), id));
            }
        });
        Self {
            pending,
            generation,
        }
    }
    pub fn submit(&self, request: T) -> u64 {
        let id = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        let (lock, notify) = &*self.pending;
        lock.lock().unwrap().0 = Some((id, request));
        notify.notify_one();
        id
    }
    pub fn cancel(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.pending.0.lock().unwrap().0 = None;
    }
    pub fn current(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }
}

impl<T> Drop for Latest<T> {
    fn drop(&mut self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        let (lock, notify) = &*self.pending;
        lock.lock().unwrap().1 = true;
        notify.notify_one();
    }
}

#[derive(Clone)]
pub struct PreviewRequest {
    pub target: PreviewTarget,
    pub explicit: bool,
    pub settings: GuiConfig,
    pub context: usize,
    pub ignore_whitespace: bool,
}

pub enum WorkerEvent {
    References(u64, Vec<Reference>, String),
    Snapshot(u64, Result<Snapshot, String>),
    History(u64, Result<HistoryPage, String>),
    Preview(u64, Result<Preview, String>),
    CommitFiles(u64, String, Result<CommitInspection, String>),
    HeadTree(u64, String, Result<Vec<FileEntry>, String>),
    CommandDone(Result<String, String>),
}

pub struct HistoryPage {
    pub commits: Vec<Commit>,
    pub exhausted: bool,
    pub elapsed_ms: u128,
}

pub struct CommandRequest {
    pub operation: Operation,
    pub cancel: Cancellation,
}

pub struct Workers {
    pub refresh: Latest<ReadOptions>,
    pub history: Latest<ReadOptions>,
    pub preview: Latest<PreviewRequest>,
    pub commit_files: Latest<String>,
    pub head_tree: Latest<String>,
    pub commands: Sender<CommandRequest>,
    pub events: Receiver<WorkerEvent>,
    pub output: Receiver<Vec<u8>>,
    _output_guard: SyncSender<Vec<u8>>,
}

impl Workers {
    pub fn new(repo: Repository) -> Self {
        let (events, receiver) = mpsc::channel();
        let (output, output_receiver) = mpsc::sync_channel(512);
        let refresh_repo = repo.clone();
        let refresh_events = events.clone();
        let refresh = Latest::spawn(move |id, options, cancel: Cancellation| {
            if let Ok((references, head)) = refresh_repo.ref_state(&cancel) {
                if !cancel.is_cancelled() {
                    let _ = refresh_events.send(WorkerEvent::References(id, references, head));
                }
            }
            let result = refresh_repo
                .status_snapshot(&options, &cancel)
                .map_err(|error| format!("{error:#}"));
            if !cancel.is_cancelled() {
                let _ = refresh_events.send(WorkerEvent::Snapshot(id, result));
            }
        });
        let history_repo = repo.clone();
        let history_events = events.clone();
        let history = Latest::spawn(move |id, mut options: ReadOptions, cancel: Cancellation| {
            let started = Instant::now();
            let limit = options.commit_limit.max(1);
            options.commit_limit = limit.saturating_add(1);
            let result = history_repo
                .commits(&options, &cancel)
                .map(|mut commits| {
                    let exhausted = commits.len() <= limit;
                    commits.truncate(limit);
                    HistoryPage {
                        commits,
                        exhausted,
                        elapsed_ms: started.elapsed().as_millis(),
                    }
                })
                .map_err(|error| format!("{error:#}"));
            if !cancel.is_cancelled() {
                let _ = history_events.send(WorkerEvent::History(id, result));
            }
        });
        let preview_repo = repo.clone();
        let preview_events = events.clone();
        let preview = Latest::spawn(move |id, request: PreviewRequest, cancel: Cancellation| {
            let result = preview_repo
                .preview(
                    &request.target,
                    request.explicit,
                    &request.settings,
                    request.context,
                    request.ignore_whitespace,
                    &cancel,
                )
                .map_err(|error| format!("{error:#}"));
            if !cancel.is_cancelled() {
                let _ = preview_events.send(WorkerEvent::Preview(id, result));
            }
        });
        let files_repo = repo.clone();
        let files_events = events.clone();
        let commit_files = Latest::spawn(move |id, oid: String, cancel: Cancellation| {
            let result = files_repo
                .inspect_commit(&oid, &cancel)
                .map_err(|error| format!("{error:#}"));
            if !cancel.is_cancelled() {
                let _ = files_events.send(WorkerEvent::CommitFiles(id, oid, result));
            }
        });
        let (commands, command_receiver) = mpsc::channel::<CommandRequest>();
        let tree_repo = repo.clone();
        let tree_events = events.clone();
        let head_tree = Latest::spawn(move |id, oid: String, cancel: Cancellation| {
            let result = tree_repo
                .tree_files(&oid, &cancel)
                .map_err(|error| format!("{error:#}"));
            if !cancel.is_cancelled() {
                let _ = tree_events.send(WorkerEvent::HeadTree(id, oid, result));
            }
        });
        let command_output = output.clone();
        thread::spawn(move || {
            while let Ok(request) = command_receiver.recv() {
                let result = repo
                    .execute(
                        &request.operation,
                        &request.cancel,
                        Some(command_output.clone()),
                    )
                    .map_err(|error| format!("{error:#}"));
                if events.send(WorkerEvent::CommandDone(result)).is_err() {
                    break;
                }
            }
        });
        Self {
            refresh,
            history,
            preview,
            commit_files,
            head_tree,
            commands,
            events: receiver,
            output: output_receiver,
            _output_guard: output,
        }
    }
}
