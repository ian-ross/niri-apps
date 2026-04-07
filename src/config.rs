use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Top-level configuration.
#[derive(Debug, Deserialize, Serialize)]
pub struct Config {
    /// Short-name aliases for application invocations,
    /// e.g. `emacs: "emacsclient -c"`.
    #[serde(default)]
    pub aliases: HashMap<String, String>,

    /// Workspaces to set up.
    #[serde(default)]
    pub workspaces: Vec<Workspace>,
}

/// Configuration for a single Niri workspace.
#[derive(Debug, Deserialize, Serialize)]
pub struct Workspace {
    /// Optional workspace name (used for reference only).
    pub name: Option<String>,

    /// When true, center all visible columns after spawning applications.
    #[serde(default)]
    pub center: bool,

    /// Columns (and standalone applications) to open on this workspace.
    #[serde(default)]
    pub columns: Vec<Column>,
}

/// A column on a workspace: one or more applications stacked vertically.
#[derive(Debug, Deserialize, Serialize)]
pub struct Column {
    /// Optional column width as a fraction of the total screen width
    /// (e.g., `0.5` for half the screen width, `1.0` for the full screen
    /// width).
    pub width: Option<f64>,

    /// When true, display the column's windows as tabs rather than stacked
    /// vertically.  The last application in the list will be the active tab.
    #[serde(default)]
    pub tabbed: bool,

    /// Applications to open in this column.
    pub apps: Vec<AppEntry>,
}

/// A single application entry within a column.
#[derive(Debug, Deserialize, Serialize)]
pub struct AppEntry {
    /// Application name or alias to launch.
    pub app: String,
}

impl Config {
    /// Load configuration from a YAML file at `path`.
    pub fn load(path: &Path) -> Result<Self> {
        let contents =
            fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let config: Config = serde_yml::from_str(&contents)
            .with_context(|| format!("parsing {}", path.display()))?;
        Ok(config)
    }

    /// Resolve an application name: if it exists in the aliases map, return
    /// the aliased command; otherwise return the name as-is.
    pub fn resolve_app<'a>(&'a self, name: &'a str) -> &'a str {
        self.aliases.get(name).map(String::as_str).unwrap_or(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_yaml() -> &'static str {
        r#"
aliases:
  emacs: "emacsclient -c"
  terminal: "foot"

workspaces:
  - name: "main"
    center: true
    columns:
      - apps:
          - app: emacs
      - width: 0.5
        apps:
          - app: terminal
          - app: "htop"
  - columns:
      - apps:
          - app: firefox
"#
    }

    #[test]
    fn parse_minimal_config() {
        let config: Config = serde_yml::from_str(minimal_yaml()).unwrap();

        assert_eq!(config.aliases.len(), 2);
        assert_eq!(config.aliases["emacs"], "emacsclient -c");
        assert_eq!(config.aliases["terminal"], "foot");

        assert_eq!(config.workspaces.len(), 2);

        let ws = &config.workspaces[0];
        assert_eq!(ws.name.as_deref(), Some("main"));
        assert!(ws.center);
        assert_eq!(ws.columns.len(), 2);
        assert_eq!(ws.columns[0].apps[0].app, "emacs");
        assert_eq!(ws.columns[1].width, Some(0.5));
        assert_eq!(ws.columns[1].apps.len(), 2);
    }

    #[test]
    fn resolve_alias() {
        let config: Config = serde_yml::from_str(minimal_yaml()).unwrap();

        assert_eq!(config.resolve_app("emacs"), "emacsclient -c");
        assert_eq!(config.resolve_app("firefox"), "firefox");
    }

    #[test]
    fn empty_config_defaults() {
        let config: Config = serde_yml::from_str("{}").unwrap();
        assert!(config.aliases.is_empty());
        assert!(config.workspaces.is_empty());
    }

    #[test]
    fn tabbed_column() {
        let yaml = r#"
workspaces:
  - columns:
      - tabbed: true
        apps:
          - app: emacs
          - app: terminal
      - apps:
          - app: firefox
"#;
        let config: Config = serde_yml::from_str(yaml).unwrap();
        let ws = &config.workspaces[0];
        assert!(ws.columns[0].tabbed);
        assert!(!ws.columns[1].tabbed);
    }
}
