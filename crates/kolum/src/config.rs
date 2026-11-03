//! `~/.config/kolum/config.toml`: profiles, the UI, the theme, the keys.
//!
//! Loading never fails. A file that does not parse, a setting serde would
//! skip, a key that does not exist: each becomes a line the TUI shows at
//! start, and the defaults stand in. A typo with no visible effect is worse
//! than an error on screen.

use crate::keymap::KeysConfig;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub default_profile: Option<String>,
    pub profiles: BTreeMap<String, Profile>,
    pub ui: UiConfig,
    pub theme: ThemeConfig,
    pub keys: KeysConfig,
    pub picker: PickerConfig,
}

/// One provider instance. Its settings beyond `provider` belong to the
/// adapter, which reads and checks them.
#[derive(Debug, Clone, Deserialize)]
pub struct Profile {
    pub provider: String,
    #[serde(flatten)]
    pub settings: toml::Table,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CardField {
    Type,
    Key,
    Priority,
    Labels,
    Epic,
    Estimate,
    Assignee,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Swimlanes {
    #[default]
    None,
    Epic,
    Assignee,
    Priority,
}

impl Swimlanes {
    /// `s` goes round them in this order.
    pub fn next(self) -> Swimlanes {
        match self {
            Swimlanes::None => Swimlanes::Epic,
            Swimlanes::Epic => Swimlanes::Assignee,
            Swimlanes::Assignee => Swimlanes::Priority,
            Swimlanes::Priority => Swimlanes::None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Swimlanes::None => "no swimlanes",
            Swimlanes::Epic => "swimlanes by epic",
            Swimlanes::Assignee => "swimlanes by assignee",
            Swimlanes::Priority => "swimlanes by priority",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    pub theme: String,
    pub hide_empty_columns: bool,
    pub swimlanes: Swimlanes,
    pub card_fields: Vec<CardField>,
    /// The right-hand column of an issue, top to bottom: built-in names, or
    /// a custom field by id (`customfield_10016`) or by name.
    pub detail_fields: Vec<String>,
}

impl Default for UiConfig {
    fn default() -> Self {
        use CardField::*;
        UiConfig {
            theme: "dark".into(),
            hide_empty_columns: false,
            swimlanes: Swimlanes::None,
            card_fields: vec![Type, Key, Priority, Labels, Epic, Estimate, Assignee],
            detail_fields: [
                "status", "assignee", "reporter", "priority", "sprint", "epic", "labels",
                "estimate", "due", "created", "updated",
            ]
            .map(String::from)
            .to_vec(),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ThemeConfig {
    /// Token by token over the named theme, as `crate::theme::Theme::apply`
    /// reads them.
    pub overrides: toml::Table,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickerLayout {
    #[default]
    Horizontal,
    Vertical,
    NoPreview,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PickerConfig {
    pub layout: PickerLayout,
    pub remote_debounce_ms: u64,
}

impl Default for PickerConfig {
    fn default() -> Self {
        PickerConfig { layout: PickerLayout::Horizontal, remote_debounce_ms: 250 }
    }
}

/// What the board toggles leave behind for the next run: `ze` and `s`.
/// Kept apart from config.toml, which is the user's file and never
/// rewritten.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, serde::Serialize)]
pub struct UiState {
    pub hide_empty_columns: bool,
    pub swimlanes: Swimlanes,
}

impl UiState {
    /// `None` when there is no state yet, or it does not read.
    pub fn load(path: &Path) -> Option<UiState> {
        toml::from_str(&std::fs::read_to_string(path).ok()?).ok()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, toml::to_string(self).map_err(std::io::Error::other)?)
    }
}

/// Reads the config, or the defaults when there is none, with every
/// problem found on the way.
pub fn load(path: &Path) -> (Config, Vec<String>) {
    match std::fs::read_to_string(path) {
        Ok(source) => parse(&source),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Config::default(), vec![]),
        Err(e) => (Config::default(), vec![format!("{}: {e}", path.display())]),
    }
}

pub fn parse(source: &str) -> (Config, Vec<String>) {
    let mut ignored = vec![];
    let parsed = toml::Deserializer::parse(source).and_then(|de| {
        serde_ignored::deserialize(de, |path| ignored.push(format!("unknown setting `{path}`")))
    });
    match parsed {
        Ok(config) => (config, ignored),
        Err(e) => {
            (Config::default(), vec![format!("config.toml does not parse, using defaults: {e}")])
        }
    }
}

/// `$XDG_CONFIG_HOME/kolum`, or `~/.config/kolum` on every platform: a
/// terminal tool's config is where its users look for one, which on macOS
/// is not `~/Library`.
pub fn config_dir() -> Option<PathBuf> {
    xdg("XDG_CONFIG_HOME", ".config")
}

pub fn cache_dir() -> Option<PathBuf> {
    xdg("XDG_CACHE_HOME", ".cache")
}

fn xdg(var: &str, fallback: &str) -> Option<PathBuf> {
    let base = std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| directories::BaseDirs::new().map(|d| d.home_dir().join(fallback)))?;
    Some(base.join("kolum"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_example_from_the_brief_parses_cleanly() {
        let (config, problems) = parse(
            r##"
            default_profile = "work"

            [profiles.work]
            provider = "jira"
            base_url = "https://acme.atlassian.net"
            email = "me@acme.com"
            token_cmd = "pass show jira/acme"
            board = 42

            [ui]
            theme = "kanagawa"
            hide_empty_columns = true
            swimlanes = "none"
            card_fields = ["type", "key", "priority", "labels", "epic", "estimate", "assignee"]
            detail_fields = ["status", "assignee", "customfield_10016"]

            [theme.overrides]
            accent = "#7e9cd8"

            [keys]
            leader = "<space>"
            timeoutlen_ms = 600

            [keys.board]
            "H" = "move_card_left"

            [keys.picker]
            "<C-t>" = "transition"

            [picker]
            layout = "horizontal"
            remote_debounce_ms = 250
            "##,
        );
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(config.default_profile.as_deref(), Some("work"));
        let work = &config.profiles["work"];
        assert_eq!(work.provider, "jira");
        assert_eq!(work.settings["board"].as_integer(), Some(42));
        assert_eq!(config.ui.theme, "kanagawa");
        assert!(config.ui.hide_empty_columns);
        assert_eq!(config.ui.detail_fields.len(), 3);
        assert_eq!(config.theme.overrides["accent"].as_str(), Some("#7e9cd8"));
        assert_eq!(config.keys.timeoutlen_ms, Some(600));
        assert!(config.keys.sections.contains_key("board"));
    }

    #[test]
    fn unknown_settings_are_reported() {
        let (config, problems) = parse("[ui]\nthme = \"x\"\nhide_empty_columns = true\n[pickr]\n");
        assert!(config.ui.hide_empty_columns, "the rest still applies");
        assert_eq!(problems, ["unknown setting `ui.thme`", "unknown setting `pickr`"]);
    }

    #[test]
    fn a_file_that_does_not_parse_falls_back_to_the_defaults() {
        let (config, problems) = parse("[ui]\nswimlanes = \"sideways\"\n");
        assert_eq!(config.ui.swimlanes, Swimlanes::None);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("sideways"), "{}", problems[0]);
    }

    #[test]
    fn the_ui_state_round_trips_and_a_bad_file_is_none() {
        let dir = std::env::temp_dir().join(format!("kolum-state-{}", std::process::id()));
        let path = dir.join("state.toml");
        assert_eq!(UiState::load(&path), None);
        let state = UiState { hide_empty_columns: true, swimlanes: Swimlanes::Assignee };
        state.save(&path).unwrap();
        assert_eq!(UiState::load(&path), Some(state));
        std::fs::write(&path, "swimlanes = 3").unwrap();
        assert_eq!(UiState::load(&path), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn no_file_is_no_problem() {
        let (config, problems) = load(Path::new("/nonexistent/kolum/config.toml"));
        assert!(problems.is_empty());
        assert_eq!(config.ui.theme, "dark");
    }
}
