use anyhow::{Context, Result, bail};
use std::{
    ffi::{OsStr, OsString},
    io::{Read, Write},
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::SyncSender,
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Default)]
pub struct Cancellation {
    generation: Option<(Arc<AtomicU64>, u64)>,
    cancelled: Arc<AtomicBool>,
}

impl Cancellation {
    pub fn generation(generation: Arc<AtomicU64>, expected: u64) -> Self {
        Self {
            generation: Some((generation, expected)),
            ..Self::default()
        }
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
            || self
                .generation
                .as_ref()
                .is_some_and(|(g, expected)| g.load(Ordering::Acquire) != *expected)
    }
    pub fn check(&self) -> Result<()> {
        if self.is_cancelled() {
            bail!("Cancelled");
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct Output {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub code: i32,
    pub truncated: bool,
}

impl Output {
    pub fn success(self) -> Result<Self> {
        if self.code != 0 {
            bail!(
                "{}",
                String::from_utf8_lossy(if self.stderr.is_empty() {
                    &self.stdout
                } else {
                    &self.stderr
                })
                .trim()
            );
        }
        Ok(self)
    }
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }
}

#[derive(Clone, Debug)]
pub struct CommandSpec {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub stdin: Option<Vec<u8>>,
    pub env: Vec<(OsString, OsString)>,
    pub label: String,
}

impl CommandSpec {
    pub fn git(args: impl IntoIterator<Item = impl Into<OsString>>) -> Self {
        let args: Vec<OsString> = args.into_iter().map(Into::into).collect();
        let label = format!(
            "git {}",
            args.iter()
                .map(|arg| shell_words::quote(&arg.to_string_lossy()).into_owned())
                .collect::<Vec<_>>()
                .join(" ")
        );
        Self {
            program: "git".into(),
            args,
            stdin: None,
            env: Vec::new(),
            label,
        }
    }
    pub fn shell(command: String) -> Self {
        #[cfg(windows)]
        let (program, args) = (
            OsString::from("cmd"),
            vec!["/C".into(), command.clone().into()],
        );
        #[cfg(not(windows))]
        let (program, args) = (
            OsString::from("sh"),
            vec!["-c".into(), command.clone().into()],
        );
        Self {
            program,
            args,
            stdin: None,
            env: Vec::new(),
            label: command,
        }
    }
    pub fn command(&self, root: &Path) -> Command {
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.args)
            .current_dir(root)
            .envs(self.env.iter().map(|(key, value)| (key, value)));
        if self.program == OsStr::new("git") {
            cmd.env("GIT_PAGER", "cat")
                .env_remove("GIT_LITERAL_PATHSPECS")
                .env("GIT_OPTIONAL_LOCKS", "0");
            // Git's stash helpers construct pathspecs internally. Literal mode belongs
            // only to explicit path arguments; --no-index uses filesystem paths.
            if self.args.iter().any(|arg| arg == OsStr::new("--"))
                && !self.args.iter().any(|arg| arg == OsStr::new("--no-index"))
            {
                cmd.env("GIT_LITERAL_PATHSPECS", "1");
            }
        }
        cmd
    }
}

fn terminate(child: &mut Child) {
    #[cfg(unix)]
    // Every managed child starts a new process group, including its hooks and SSH children.
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/T", "/F", "/PID", &child.id().to_string()])
            .output();
    }
    let _ = child.kill();
}

fn read_pipe(
    mut pipe: impl Read,
    limit: usize,
    overflow: Arc<AtomicBool>,
    stream: Option<SyncSender<Vec<u8>>>,
) -> std::io::Result<Vec<u8>> {
    let mut result = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let request = chunk
            .len()
            .min(limit.saturating_sub(result.len()).saturating_add(1));
        let count = pipe.read(&mut chunk[..request])?;
        if count == 0 {
            break;
        }
        if let Some(stream) = &stream {
            let _ = stream.try_send(chunk[..count].to_vec());
        }
        let remaining = limit.saturating_sub(result.len());
        result.extend_from_slice(&chunk[..count.min(remaining)]);
        if count > remaining {
            overflow.store(true, Ordering::Release);
            break;
        }
    }
    Ok(result)
}

pub fn run(
    root: &Path,
    spec: &CommandSpec,
    cancel: &Cancellation,
    limit: usize,
    stream: Option<SyncSender<Vec<u8>>>,
    timeout: Duration,
) -> Result<Output> {
    cancel.check()?;
    let mut cmd = spec.command(root);
    if stream.is_some() && spec.program == OsStr::new("git") {
        // LFS otherwise suppresses transfer updates when Git's output is piped.
        cmd.env("GIT_LFS_FORCE_PROGRESS", "1");
    }
    cmd.stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(if spec.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd
        .spawn()
        .with_context(|| format!("Unable to run {}", spec.label))?;
    let overflow = Arc::new(AtomicBool::new(false));
    let stdout = child.stdout.take().context("Missing command stdout")?;
    let stderr = child.stderr.take().context("Missing command stderr")?;
    let stdout_overflow = overflow.clone();
    let stderr_overflow = overflow.clone();
    let stderr_stream = stream.clone();
    let stdout_reader = thread::spawn(move || read_pipe(stdout, limit, stdout_overflow, stream));
    let stderr_reader =
        thread::spawn(move || read_pipe(stderr, limit, stderr_overflow, stderr_stream));
    let writer = spec.stdin.as_ref().map(|data| {
        let data = data.clone();
        let mut input = child.stdin.take().expect("piped stdin");
        thread::spawn(move || input.write_all(&data))
    });
    let started = Instant::now();
    let mut timed_out = false;
    let mut exited = None;
    let status = loop {
        if cancel.is_cancelled() || overflow.load(Ordering::Acquire) || started.elapsed() > timeout
        {
            timed_out = started.elapsed() > timeout;
            terminate(&mut child);
            break child.wait()?;
        }
        if exited.is_none() {
            exited = child.try_wait()?;
        }
        if let Some(status) = exited {
            if stdout_reader.is_finished() && stderr_reader.is_finished() {
                break status;
            }
        }
        thread::sleep(Duration::from_millis(4));
    };
    let stdout = stdout_reader
        .join()
        .map_err(|_| anyhow::anyhow!("stdout reader panicked"))??;
    let stderr = stderr_reader
        .join()
        .map_err(|_| anyhow::anyhow!("stderr reader panicked"))??;
    if let Some(writer) = writer {
        let _ = writer.join();
    }
    cancel.check()?;
    if timed_out {
        bail!(
            "Command timed out after {} seconds: {}",
            timeout.as_secs(),
            spec.label
        );
    }
    Ok(Output {
        stdout,
        stderr,
        code: status.code().unwrap_or(-1),
        truncated: overflow.load(Ordering::Acquire),
    })
}

pub fn os_path(bytes: &[u8]) -> OsString {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        OsString::from_vec(bytes.to_vec())
    }
    #[cfg(not(unix))]
    {
        String::from_utf8_lossy(bytes).into_owned().into()
    }
}

pub fn path_bytes(path: &Path) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().to_vec()
    }
    #[cfg(not(unix))]
    {
        path.to_string_lossy().as_bytes().to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn bounded_reader_consumes_at_most_the_limit_plus_one() {
        let mut source = Cursor::new(vec![b'x'; 100_000]);
        let overflow = Arc::new(AtomicBool::new(false));
        let data = read_pipe(&mut source, 100, overflow.clone(), None).unwrap();
        assert_eq!(data.len(), 100);
        assert_eq!(source.position(), 101);
        assert!(overflow.load(Ordering::Acquire));
    }
}
