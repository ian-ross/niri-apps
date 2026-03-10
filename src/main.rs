mod config;
mod ipc;

use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result};
use clap::Parser;

/// Declarative application spawner for the Niri Wayland compositor.
///
/// Reads a YAML configuration file describing which applications to start on
/// which workspaces, spawns them, and uses Niri's IPC interface to arrange
/// them as requested.
#[derive(Debug, Parser)]
#[command(author, version, about)]
struct Cli {
    /// Path to the configuration file (defaults to
    /// `$XDG_CONFIG_HOME/niri-apps/config.yaml` or
    /// `~/.config/niri-apps/config.yaml`).
    #[arg(short, long, value_name = "FILE")]
    config: Option<PathBuf>,
}

fn default_config_path() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            PathBuf::from(home).join(".config")
        });
    base.join("niri-apps").join("config.yaml")
}

fn run(cli: Cli) -> Result<()> {
    let config_path = cli.config.unwrap_or_else(default_config_path);
    let config = config::Config::load(&config_path)
        .with_context(|| format!("loading config from {}", config_path.display()))?;

    for (workspace_index, workspace) in config.workspaces.iter().enumerate() {
        // Workspaces in Niri are 1-based.
        let ws_index = (workspace_index + 1) as u8;

        ipc::focus_workspace(ws_index).with_context(|| format!("focusing workspace {ws_index}"))?;

        for column in &workspace.columns {
            for entry in &column.apps {
                let command = config.resolve_app(&entry.app);
                spawn_app(command).with_context(|| format!("spawning application '{command}'"))?;
            }
        }

        if workspace.center {
            ipc::center_visible_columns().context("centering visible columns")?;
        }
    }

    Ok(())
}

/// Parse a shell-style command string and spawn it as a detached process.
fn spawn_app(command: &str) -> Result<()> {
    let mut parts = shell_words(command);
    if parts.is_empty() {
        anyhow::bail!("empty command string");
    }
    let program = parts.remove(0);
    Command::new(&program)
        .args(&parts)
        .spawn()
        .with_context(|| format!("spawning '{program}'"))?;
    Ok(())
}

/// Very small shell-word splitter: splits on whitespace, respecting
/// double-quoted spans. Good enough for simple application invocations.
fn shell_words(s: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;

    for ch in s.chars() {
        match ch {
            '"' => in_quotes = !in_quotes,
            ' ' | '\t' if !in_quotes => {
                if !current.is_empty() {
                    words.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(ch),
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

fn main() {
    let cli = Cli::parse();
    if let Err(err) = run(cli) {
        eprintln!("Error: {err:#}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_words_simple() {
        assert_eq!(shell_words("emacsclient -c"), vec!["emacsclient", "-c"]);
    }

    #[test]
    fn shell_words_quoted() {
        assert_eq!(
            shell_words(r#"bash -c "echo hello world""#),
            vec!["bash", "-c", "echo hello world"]
        );
    }

    #[test]
    fn shell_words_empty() {
        assert!(shell_words("").is_empty());
        assert!(shell_words("   ").is_empty());
    }

    #[test]
    fn default_config_path_contains_niri_apps() {
        let path = default_config_path();
        let s = path.to_string_lossy();
        assert!(s.contains("niri-apps"));
        assert!(s.ends_with("config.yaml"));
    }
}
