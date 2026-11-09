use anyhow::{Context, Result, bail};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::{Deserialize, Serialize};
use serde_yaml::Value;
use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GuiConfig {
    pub show_unchanged_files: bool,
    pub show_file_tree: bool,
    pub diff_preview_max_file_bytes: usize,
    pub diff_preview_max_bytes: usize,
    pub side_panel_width: f64,
    pub mouse_events: bool,
    pub command_log_size: usize,
}

impl Default for GuiConfig {
    fn default() -> Self {
        Self {
            show_unchanged_files: true,
            show_file_tree: true,
            diff_preview_max_file_bytes: 2 * 1024 * 1024,
            diff_preview_max_bytes: 512 * 1024,
            side_panel_width: 0.32,
            mouse_events: true,
            command_log_size: 1000,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GitConfig {
    pub auto_refresh: bool,
    pub auto_fetch: bool,
}
impl Default for GitConfig {
    fn default() -> Self {
        Self {
            auto_refresh: true,
            auto_fetch: true,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RefresherConfig {
    pub refresh_interval: u64,
    pub fetch_interval: u64,
}
impl Default for RefresherConfig {
    fn default() -> Self {
        Self {
            refresh_interval: 3,
            fetch_interval: 600,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct OsConfig {
    pub edit: String,
    pub edit_at_line: String,
    pub open: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CustomCommand {
    pub key: String,
    pub context: String,
    pub command: String,
    pub description: String,
    pub loading_text: String,
    pub subprocess: bool,
    pub prompts: Vec<Value>,
}
impl Default for CustomCommand {
    fn default() -> Self {
        Self {
            key: String::new(),
            context: "global".into(),
            command: String::new(),
            description: String::new(),
            loading_text: String::new(),
            subprocess: false,
            prompts: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    pub gui: GuiConfig,
    pub git: GitConfig,
    pub refresher: RefresherConfig,
    pub os: OsConfig,
    pub custom_commands: Vec<CustomCommand>,
    pub keybinding: BTreeMap<String, BTreeMap<String, Value>>,
    pub confirm_on_quit: bool,
    #[serde(skip)]
    pub directory: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct State {
    pub diff_preview_enabled: bool,
    pub show_ignored: bool,
    pub recent_repos: Vec<PathBuf>,
    pub hide_command_log: bool,
    pub visible_branches: BTreeMap<String, Vec<String>>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            diff_preview_enabled: true,
            show_ignored: false,
            recent_repos: Vec::new(),
            hide_command_log: true,
            visible_branches: BTreeMap::new(),
        }
    }
}

pub fn default_directory() -> PathBuf {
    if let Some(dir) = env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(dir).join("pd-lazygit-rust");
    }
    #[cfg(windows)]
    if let Some(dir) = env::var_os("APPDATA") {
        return PathBuf::from(dir).join("pd-lazygit-rust");
    }
    PathBuf::from(env::var_os("HOME").unwrap_or_else(|| ".".into())).join(".config/pd-lazygit-rust")
}

fn merge(base: &mut Value, overlay: Value) {
    match (base, overlay) {
        (Value::Mapping(base), Value::Mapping(overlay)) => {
            for (key, value) in overlay {
                merge(base.entry(key).or_insert(Value::Null), value);
            }
        }
        (base, overlay) => *base = overlay,
    }
}

impl Config {
    pub fn load(directory: Option<PathBuf>, files: &[PathBuf]) -> Result<Self> {
        let directory = directory.unwrap_or_else(default_directory);
        let mut value = Value::Mapping(Default::default());
        let default_file = directory.join("config.yml");
        let mut paths = Vec::new();
        if default_file.is_file() {
            paths.push(default_file);
        }
        paths.extend_from_slice(files);
        for path in paths {
            let text =
                fs::read_to_string(&path).with_context(|| format!("Reading {}", path.display()))?;
            let overlay = serde_yaml::from_str(&text)
                .with_context(|| format!("Invalid YAML in {}", path.display()))?;
            merge(&mut value, overlay);
        }
        let mut config: Self = serde_yaml::from_value(value).context("Invalid configuration")?;
        if !(0.1..=0.8).contains(&config.gui.side_panel_width) {
            bail!("gui.sidePanelWidth must be between 0.1 and 0.8");
        }
        config.refresher.refresh_interval = config.refresher.refresh_interval.max(1);
        config.refresher.fetch_interval = config.refresher.fetch_interval.max(30);
        config.gui.command_log_size = config.gui.command_log_size.clamp(100, 100_000);
        config.directory = directory;
        Ok(config)
    }
    pub fn binding(&self, group: &str, action: &str, fallback: &[&str], key: KeyEvent) -> bool {
        if let Some(value) = self.keybinding.get(group).and_then(|g| g.get(action)) {
            match value {
                Value::String(value) => key_matches(value, key),
                Value::Sequence(values) => values
                    .iter()
                    .any(|value| value.as_str().is_some_and(|value| key_matches(value, key))),
                _ => false,
            }
        } else {
            fallback.iter().any(|binding| key_matches(binding, key))
        }
    }
}

impl State {
    pub fn load(directory: &Path) -> Result<Self> {
        let path = directory.join("rust-state.json");
        if path.exists() {
            return serde_json::from_slice(&fs::read(path)?).context("Invalid rust-state.json");
        }
        let legacy = directory.join("state.yml");
        if legacy.is_file() {
            let state = serde_yaml::from_slice(&fs::read(legacy)?).context("Invalid state.yml")?;
            return Ok(state);
        }
        Ok(Self::default())
    }
    pub fn save(&self, directory: &Path) -> Result<()> {
        fs::create_dir_all(directory)?;
        let temporary = directory.join(format!(".rust-state.{}.json", std::process::id()));
        fs::write(&temporary, serde_json::to_vec_pretty(self)?)?;
        fs::rename(temporary, directory.join("rust-state.json"))?;
        Ok(())
    }
    pub fn remember(&mut self, root: &Path) {
        self.recent_repos.retain(|path| path != root);
        self.recent_repos.insert(0, root.to_path_buf());
        self.recent_repos.truncate(25);
    }
}

pub fn key_matches(binding: &str, key: KeyEvent) -> bool {
    if binding.is_empty() || binding == "<disabled>" {
        return false;
    }
    if !binding.starts_with('<') || !binding.ends_with('>') {
        let mut chars = binding.chars();
        return chars.next().is_some_and(|c| key.code == KeyCode::Char(c))
            && chars.next().is_none()
            && !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER);
    }
    let inner = &binding[1..binding.len() - 1];
    let parts: Vec<_> = inner.split('+').collect();
    let mut modifiers = KeyModifiers::empty();
    for modifier in &parts[..parts.len().saturating_sub(1)] {
        modifiers |= match *modifier {
            "ctrl" => KeyModifiers::CONTROL,
            "alt" => KeyModifiers::ALT,
            "shift" => KeyModifiers::SHIFT,
            "meta" | "super" => KeyModifiers::SUPER,
            _ => return false,
        };
    }
    let name = parts.last().copied().unwrap_or("");
    let code = match name {
        "space" => KeyCode::Char(' '),
        "enter" => KeyCode::Enter,
        "esc" => KeyCode::Esc,
        "tab" => KeyCode::Tab,
        "backtab" => {
            modifiers |= KeyModifiers::SHIFT;
            KeyCode::BackTab
        }
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "pgup" => KeyCode::PageUp,
        "pgdown" => KeyCode::PageDown,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "backspace" => KeyCode::Backspace,
        "delete" => KeyCode::Delete,
        name if name.chars().count() == 1 => KeyCode::Char(name.chars().next().unwrap()),
        _ => return false,
    };
    let actual_modifiers = if matches!(code, KeyCode::Char(c) if c.is_uppercase()) {
        key.modifiers - KeyModifiers::SHIFT
    } else {
        key.modifiers
    };
    key.code == code && actual_modifiers == modifiers
}
