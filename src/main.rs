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

    // Connect to the niri event stream.  The initial snapshot gives us the
    // current workspace list (to compute the starting index) and the set of
    // already-open window ids (so we can detect each newly spawned window).
    let (mut event_stream, workspaces, mut known_ids) =
        ipc::EventStream::connect().context("connecting to niri event stream")?;

    // Find the empty workspace at the bottom of the current output's stack
    // and use it as our starting point, so that we don't disturb existing
    // windows.
    let focused_output = workspaces
        .iter()
        .find(|ws| ws.is_focused)
        .and_then(|ws| ws.output.as_ref());
    let start_index = if let Some(output) = focused_output {
        workspaces
            .iter()
            .filter(|ws| ws.output.as_ref() == Some(output) && ws.active_window_id.is_none())
            .map(|ws| ws.idx)
            .max()
            .unwrap_or(1)
    } else {
        1
    };

    for (workspace_index, workspace) in config.workspaces.iter().enumerate() {
        let ws_index = start_index + workspace_index as u8;

        // Ask niri to focus the workspace (creating it if it doesn't exist yet).
        ipc::focus_workspace(ws_index).with_context(|| format!("focusing workspace {ws_index}"))?;

        // Wait for the compositor to confirm the focus via the event stream.
        // This also resolves ws_index to its stable id, which is needed to
        // correctly identify windows that open on this workspace.
        let ws_id = event_stream
            .wait_for_workspace_focus(ws_index)
            .with_context(|| format!("waiting for workspace {ws_index} to be focused"))?;

        for column in &workspace.columns {
            for (app_index, entry) in column.apps.iter().enumerate() {
                let command = config.resolve_app(&entry.app);

                spawn_app(command)
                    .with_context(|| format!("spawning application '{command}'"))?;

                // Wait for the new window to appear on this workspace.  We
                // always wait (not just for non-first apps or width-bearing
                // columns) so that the compositor has fully settled before we
                // move on — this prevents windows from opening on the wrong
                // workspace when multiple workspaces are being set up.
                let new_id = event_stream
                    .wait_for_new_window(&known_ids, ws_id)
                    .with_context(|| {
                        format!("waiting for window of '{command}' to appear")
                    })?;
                known_ids.insert(new_id);

                // For non-first apps in a column, pull the window into the
                // column to its left.
                if app_index > 0 {
                    ipc::consume_or_expel_window_left()
                        .context("consuming window into column")?;
                }
            }

            // Apply the column width if one is specified.  The config uses
            // fractions of the display width (e.g. 0.5 = half-width), while
            // the Niri IPC SetProportion action expects a percentage value
            // (e.g. 50.0), so we multiply by 100.
            if let Some(width) = column.width {
                ipc::set_column_width(niri_ipc::SizeChange::SetProportion(width * 100.0))
                    .context("setting column width")?;
            }
        }

        if workspace.center {
            ipc::focus_column_first().context("focusing first column before centering")?;
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
