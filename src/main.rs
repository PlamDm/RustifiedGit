use anyhow::{Context, Result, bail};
use clap::Parser;
use crossterm::{
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
    },
    execute,
};
use pd_lazygit::{
    app::App,
    config::{Config, State},
    git::{HISTORY_PAGE_SIZE, ReadOptions, Repository},
    model::Panel,
    process::Cancellation,
    ui,
};
use ratatui::{DefaultTerminal, Terminal, backend::TestBackend};
use std::{
    io::{self, IsTerminal},
    path::PathBuf,
    time::{Duration, Instant},
};

#[derive(Parser)]
#[command(
    name = "pd-lazygit",
    version,
    about = "Rust port of PlamDm/pd_lazygit",
    long_about = "A native Rust Git TUI with your fork's repository browsing, bounded previews, file history, external tools, and live command output."
)]
struct Args {
    #[arg(short = 'p', long = "path", default_value = ".")]
    path: PathBuf,
    #[arg(long)]
    use_config_dir: Option<PathBuf>,
    #[arg(long, value_delimiter = ',')]
    use_config_file: Vec<PathBuf>,
    #[arg(
        long,
        help = "Print repository state as JSON without opening a terminal"
    )]
    snapshot: bool,
    #[arg(
        long,
        help = "Measure cold/warm repository snapshots without changing the repository"
    )]
    benchmark: bool,
    #[arg(long, default_value_t = 20)]
    iterations: usize,
    #[arg(
        long,
        help = "Render a deterministic terminal screen, for example --render 120x40"
    )]
    render: Option<String>,
    #[arg(long, help = "Filter history by repository-relative path")]
    filter: Option<PathBuf>,
    #[arg(long, help = "Validate YAML configuration and exit")]
    validate_config: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let config = Config::load(args.use_config_dir, &args.use_config_file)?;
    if args.validate_config {
        println!("Configuration valid: {}", config.directory.display());
        return Ok(());
    }
    let repo = Repository::discover(&args.path)?;
    let history_directory = args
        .filter
        .as_ref()
        .is_some_and(|path| repo.root.join(path).is_dir());
    let options = ReadOptions {
        history_path: args.filter,
        history_directory,
        commit_limit: HISTORY_PAGE_SIZE,
        ..ReadOptions::default()
    };
    if args.snapshot {
        println!(
            "{}",
            serde_json::to_string_pretty(&repo.snapshot(&options, &Cancellation::default())?)?
        );
        return Ok(());
    }
    if args.benchmark {
        let count = args.iterations.clamp(1, 1000);
        let mut times = Vec::with_capacity(count);
        let mut last = Default::default();
        for _ in 0..count {
            let start = Instant::now();
            last = repo.snapshot(&options, &Cancellation::default())?;
            times.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        let cold = times[0];
        let mut sorted = times.clone();
        sorted.sort_by(f64::total_cmp);
        println!(
            "{}",
            serde_json::json!({ "repository": repo.root, "iterations": count, "cold_ms": cold, "median_ms": sorted[sorted.len()/2], "p95_ms": sorted[((sorted.len() as f64 * 0.95).ceil() as usize).saturating_sub(1)], "min_ms": sorted[0], "files": last.files.len(), "commits_loaded": last.commits.len(), "references": last.references.len() })
        );
        return Ok(());
    }
    if args.render.is_none() && (!io::stdin().is_terminal() || !io::stdout().is_terminal()) {
        bail!(
            "An interactive terminal is required; use --snapshot or --render for headless operation"
        );
    }
    let state = State::load(&config.directory)?;
    let mut app = App::new(repo, config, state);
    app.options.history_path = options.history_path;
    app.options.history_directory = options.history_directory;
    if app.options.history_path.is_some() {
        app.workers.refresh.cancel();
        app.loading = false;
        app.refresh();
    }
    if let Some(size) = args.render {
        let (width, height) = size.split_once('x').context("Use --render WIDTHxHEIGHT")?;
        let (width, height) = (width.parse::<u16>()?, height.parse::<u16>()?);
        if width > 1000 || height > 1000 {
            bail!("Render dimensions must not exceed 1000");
        }
        app.apply_snapshot(app.repo.snapshot(&app.options, &Cancellation::default())?);
        app.loading = false;
        app.preview = app.repo.preview(
            &pd_lazygit::model::PreviewTarget::Status,
            false,
            &app.config.gui,
            3,
            false,
            &Cancellation::default(),
        )?;
        app.preview_lines = app.preview.text.lines().map(str::to_owned).collect();
        let mut terminal = Terminal::new(TestBackend::new(width, height))?;
        terminal.draw(|frame| ui::draw(frame, &mut app))?;
        let buffer = terminal.backend().buffer();
        for y in 0..height {
            let line: String = (0..width).map(|x| buffer[(x, y)].symbol()).collect();
            println!("{}", line.trim_end());
        }
        return Ok(());
    }
    let mut terminal = initialize(&app)?;
    let result = run(&mut terminal, &mut app);
    restore();
    if let Some((cancel, _, _)) = &app.busy {
        cancel.cancel();
    }
    let save = app.state.save(&app.config.directory);
    result?;
    save?;
    Ok(())
}

fn initialize(app: &App) -> Result<DefaultTerminal> {
    let terminal = ratatui::try_init()?;
    execute!(io::stdout(), EnableBracketedPaste)?;
    if app.config.gui.mouse_events {
        execute!(io::stdout(), EnableMouseCapture)?;
    }
    Ok(terminal)
}

fn restore() {
    let _ = execute!(io::stdout(), DisableMouseCapture, DisableBracketedPaste);
    ratatui::restore();
}

fn run(terminal: &mut DefaultTerminal, app: &mut App) -> Result<()> {
    while !app.quit {
        app.poll_workers();
        if app.config.git.auto_refresh
            && !app.loading
            && app.busy.is_none()
            && app.last_refresh.elapsed()
                >= Duration::from_secs(app.config.refresher.refresh_interval)
        {
            app.refresh();
        }
        if app.config.git.auto_fetch
            && app.busy.is_none()
            && app.last_fetch.elapsed() >= Duration::from_secs(app.config.refresher.fetch_interval)
        {
            app.run_operation(pd_lazygit::git::Operation::git([
                "fetch",
                "--all",
                "--prune",
                "--progress",
            ]));
            app.last_fetch = Instant::now();
        }
        if app.busy.is_some() {
            app.dirty = true;
        }
        if app.dirty {
            terminal.draw(|frame| ui::draw(frame, app))?;
            app.dirty = false;
        }
        if let Some(command) = app.pending_external.take() {
            if app.busy.is_some() {
                app.error("Wait for the active command, or cancel it with Ctrl+c.".into());
            } else {
                app.workers.preview.cancel();
                app.workers.refresh.cancel();
                app.loading = false;
                restore();
                let result = command.command(&app.repo.root).status();
                *terminal = initialize(app)?;
                terminal.clear()?;
                match result {
                    Ok(status) if status.success() => {}
                    Ok(status) => app.error(format!("{} exited with {status}", command.label)),
                    Err(error) => app.error(error.to_string()),
                }
                app.refresh();
                app.dirty = true;
            }
        }
        if let Some(path) = app.pending_repo.take() {
            if app.busy.is_some() {
                app.error("Wait for the active command before switching repositories.".into());
            } else {
                match Repository::discover(&path) {
                    Ok(repo) => {
                        app.state.save(&app.config.directory)?;
                        let state = app.state.clone();
                        let config = app.config.clone();
                        *app = App::new(repo, config, state);
                    }
                    Err(error) => app.error(format!("{error:#}")),
                }
            }
        }
        if event::poll(Duration::from_millis(
            if app.loading
                || app.history_loading
                || app.inspection_loading
                || app.preview_loading
                || app.busy.is_some()
            {
                16
            } else {
                40
            },
        ))? {
            match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => app.handle_key(key),
                Event::Resize(_, _) => {
                    app.dirty = true;
                }
                Event::Paste(text) => app.paste(text),
                Event::Mouse(mouse) if app.modal.is_none() => match mouse.kind {
                    MouseEventKind::Down(MouseButton::Left) => {
                        let size = terminal.size()?;
                        if let Some((panel, index)) = ui::hit_test(
                            ratatui::layout::Rect::new(0, 0, size.width, size.height),
                            app,
                            mouse.column,
                            mouse.row,
                        ) {
                            if panel == Panel::Diff {
                                app.set_focus(panel);
                                app.diff_line =
                                    index.min(app.preview_lines.len().saturating_sub(1));
                            } else if matches!(panel, Panel::Output | Panel::Details) {
                                app.set_focus(panel);
                            } else {
                                app.select_list_row(panel, index);
                            }
                        }
                    }
                    MouseEventKind::ScrollDown => {
                        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE))
                    }
                    MouseEventKind::ScrollUp => {
                        app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE))
                    }
                    _ => {}
                },
                _ => {}
            }
        }
    }
    Ok(())
}
