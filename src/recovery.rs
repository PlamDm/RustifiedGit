//! Selectable command diagnostics. Output is never interpreted by a shell.
use crate::{
    git::{Operation, Step},
    process::CommandSpec,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
};

#[derive(Default)]
pub struct MessageSelection {
    pub cursor: usize,
    pub marked: BTreeSet<usize>,
    pub commands: BTreeMap<usize, CommandSpec>,
    pub row_starts: Vec<usize>,
    pub follow_cursor: bool,
}

/// Only complete, standalone command lines are executable. Shell constructs and
/// placeholders remain ordinary, copyable text instead of changing meaning.
pub fn command_line(line: &str) -> Option<CommandSpec> {
    let mut text = line.trim();
    if let Some(rest) = text.strip_prefix("hint:") {
        text = rest.trim();
    }
    let explicit = text.starts_with("$ ") || text.starts_with('`');
    text = text.strip_prefix("$ ").unwrap_or(text);
    if text.starts_with('`') && text.ends_with('`') && text.len() > 1 {
        text = &text[1..text.len() - 1];
    }
    if text.chars().any(char::is_control) {
        return None;
    }
    if text.contains(['\n', '\r', '`', '$', ';', '|', '&', '<', '>']) {
        return None;
    }
    let mut words = shell_words::split(text).ok()?;
    if words.is_empty() || words.iter().any(|word| word == "#" || word == "...") {
        return None;
    }
    if !explicit
        && !matches!(
            words[0].as_str(),
            "git" | "gpg" | "gpgconf" | "gh" | "ssh-add"
        )
    {
        return None;
    }
    let program = words.remove(0);
    if program.contains('=') || program.starts_with('-') || words.is_empty() {
        return None;
    }
    if program == "git"
        && words
            .iter()
            .any(|word| word == "user.email" || word == "user.name")
        && words
            .last()
            .is_some_and(|word| matches!(word.as_str(), "you@example.com" | "Your Name"))
    {
        return None;
    }
    let args: Vec<OsString> = words.into_iter().map(Into::into).collect();
    if program == "git" {
        return Some(CommandSpec::git(args));
    }
    let label = std::iter::once(program.as_str())
        .chain(args.iter().map(|s| s.to_str().unwrap()))
        .map(|s| shell_words::quote(s).into_owned())
        .collect::<Vec<_>>()
        .join(" ");
    Some(CommandSpec {
        program: program.into(),
        args,
        stdin: None,
        env: Vec::new(),
        label,
    })
}

impl MessageSelection {
    pub fn prepare(message: String) -> (String, Self) {
        let mut lines: Vec<String> = message.split('\n').map(str::to_owned).collect();
        let lower = message.to_lowercase();
        let suggestions: &[(&str, &str)] = if lower.contains("divergent branches")
            || lower.contains("specify how to reconcile")
        {
            &[
                (
                    "Rebase local commits onto the upstream (choose one pull method)",
                    "git pull --rebase",
                ),
                (
                    "Merge the upstream (choose one pull method)",
                    "git pull --no-rebase",
                ),
                (
                    "Pull only if a fast-forward is possible",
                    "git pull --ff-only",
                ),
            ]
        } else if lower.contains("would be overwritten")
            || lower.contains("cannot pull with rebase: you have unstaged changes")
        {
            &[(
                "Save tracked and untracked changes before retrying the original command",
                "git stash push --include-untracked",
            )]
        } else if lower.contains("non-fast-forward") || lower.contains("fetch first") {
            &[(
                "Fetch remote changes before inspecting and reconciling the histories",
                "git fetch --prune",
            )]
        } else if lower.contains("unmerged files") || lower.contains("resolve your current index") {
            &[(
                "Inspect conflicts; resolve them before retrying",
                "git status",
            )]
        } else if lower.contains("gpg failed to sign")
            || lower.contains("failed to sign the data")
            || lower.contains("no secret key")
        {
            &[
                (
                    "List available secret signing keys",
                    "gpg --list-secret-keys --keyid-format=long",
                ),
                (
                    "Inspect the configured signing key",
                    "git config --show-origin --get user.signingkey",
                ),
                (
                    "Inspect signing configuration",
                    "git config --show-origin --get commit.gpgsign",
                ),
            ]
        } else {
            &[]
        };
        let mut state = Self::default();
        for (index, line) in lines.iter().enumerate() {
            if let Some(command) = command_line(line) {
                state.commands.insert(index, command);
            }
        }
        for (description, text) in suggestions {
            let command = command_line(text).unwrap();
            if state
                .commands
                .values()
                .any(|existing| existing.label == command.label)
            {
                continue;
            }
            lines.extend([String::new(), format!("Suggested: {description}")]);
            state.commands.insert(lines.len(), command);
            lines.push((*text).into());
        }
        (lines.join("\n"), state)
    }

    pub fn toggle(&mut self) {
        if !self.marked.remove(&self.cursor) {
            self.marked.insert(self.cursor);
        }
    }

    pub fn copy_text(&self, message: &str) -> String {
        if self.marked.is_empty() {
            return message.to_owned();
        }
        message
            .split('\n')
            .enumerate()
            .filter(|(index, _)| self.marked.contains(index))
            .map(|(_, text)| text)
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn operation(&self) -> Option<Operation> {
        let commands: Vec<_> = self
            .marked
            .iter()
            .filter_map(|index| self.commands.get(index))
            .cloned()
            .collect();
        if commands.is_empty() {
            return None;
        }
        Some(Operation {
            label: commands
                .iter()
                .map(|command| command.label.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
            steps: commands.into_iter().map(Step::Command).collect(),
        })
    }
}
