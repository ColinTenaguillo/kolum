//! Colours as semantic tokens. Widgets ask for `muted` or `status.done`,
//! never for a colour, so a theme (built in, from a file, or overridden one
//! token at a time in the config) restyles everything at once.

use kolum_domain::{IssueKind, Priority, StatusCategory};
use ratatui::style::Color;
use std::path::Path;
use std::str::FromStr;

macro_rules! tokens {
    ($($field:ident $name:literal,)*) => {
        #[derive(Debug, Clone, PartialEq)]
        pub struct Theme {
            $(pub $field: Color,)*
            /// Labels take a colour from here, by a hash of their name, so a
            /// label keeps its colour from one card and one day to the next.
            pub label_palette: Vec<Color>,
        }

        impl Theme {
            /// Every token name the config may override.
            pub const TOKENS: &[&str] = &[$($name,)* "label_palette"];

            fn token_mut(&mut self, name: &str) -> Option<&mut Color> {
                match name {
                    $($name => Some(&mut self.$field),)*
                    _ => None,
                }
            }

            /// A theme from its colours, in the order the tokens are declared.
            fn from_hex(c: [u32; tokens!(@count $($field)*)], labels: &[u32]) -> Self {
                let mut i = 0..;
                let mut next = || rgb(c[i.next().unwrap_or(0)]);
                Theme { $($field: next(),)* label_palette: labels.iter().copied().map(rgb).collect() }
            }
        }
    };
    (@count $($t:ident)*) => { [$(tokens!(@one $t)),*].len() };
    (@one $t:ident) => { () };
}

tokens! {
    bg "bg",
    fg "fg",
    muted "muted",
    accent "accent",
    border "border",
    border_focus "border_focus",
    selection "selection",
    match_highlight "match_highlight",
    status_todo "status.todo",
    status_doing "status.doing",
    status_done "status.done",
    priority_highest "priority.highest",
    priority_high "priority.high",
    priority_medium "priority.medium",
    priority_low "priority.low",
    priority_lowest "priority.lowest",
    type_bug "type.bug",
    type_story "type.story",
    type_task "type.task",
    type_epic "type.epic",
    type_subtask "type.subtask",
    error "error",
    warn "warn",
    ok "ok",
}

fn rgb(hex: u32) -> Color {
    Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

pub const BUILTIN: &[&str] = &["dark", "light", "kanagawa", "catppuccin", "tokyonight", "gruvbox"];

impl Theme {
    // Columns: bg fg muted accent border border_focus selection match,
    // status todo/doing/done, priority highest..lowest,
    // type bug/story/task/epic/subtask, error warn ok.
    #[rustfmt::skip]
    pub fn builtin(name: &str) -> Option<Theme> {
        Some(match name {
            "dark" => Theme::from_hex([
                0x121212, 0xd4d4d4, 0x808080, 0x5fafff, 0x3a3a3a, 0x5fafff, 0x264f78, 0xffaf00,
                0x8a8a8a, 0x5fafff, 0x5fd75f, 0xff5f5f, 0xff8700, 0xffd75f, 0x5fafd7, 0x8a8a8a,
                0xff5f5f, 0x5fd75f, 0x5fafff, 0xaf87ff, 0x87afd7, 0xff5f5f, 0xffaf00, 0x5fd75f,
            ], &[0x5f87af, 0x5faf87, 0xaf875f, 0x875faf, 0xaf5f87, 0x5fafaf, 0x87af5f, 0xaf5f5f]),
            "light" => Theme::from_hex([
                0xfafafa, 0x24292f, 0x6e7781, 0x0969da, 0xd0d7de, 0x0969da, 0xddf4ff, 0xbf8700,
                0x6e7781, 0x0969da, 0x1a7f37, 0xcf222e, 0xbc4c00, 0x9a6700, 0x0969da, 0x6e7781,
                0xcf222e, 0x1a7f37, 0x0969da, 0x8250df, 0x54aeff, 0xcf222e, 0x9a6700, 0x1a7f37,
            ], &[0x0969da, 0x1a7f37, 0x8250df, 0xbc4c00, 0xbf3989, 0x1b7c83, 0x9a6700, 0x57606a]),
            "kanagawa" => Theme::from_hex([
                0x1f1f28, 0xdcd7ba, 0x727169, 0x7e9cd8, 0x363646, 0x7e9cd8, 0x2d4f67, 0xffa066,
                0x727169, 0x7e9cd8, 0x98bb6c, 0xe82424, 0xff9e3b, 0xe6c384, 0x7fb4ca, 0x727169,
                0xe46876, 0x98bb6c, 0x7e9cd8, 0x957fb8, 0x7fb4ca, 0xe82424, 0xff9e3b, 0x98bb6c,
            ], &[0x7e9cd8, 0x98bb6c, 0x957fb8, 0xffa066, 0xd27e99, 0x7aa89f, 0xe6c384, 0x938aa9]),
            "catppuccin" => Theme::from_hex([
                0x1e1e2e, 0xcdd6f4, 0x6c7086, 0x89b4fa, 0x313244, 0x89b4fa, 0x45475a, 0xfab387,
                0x7f849c, 0x89b4fa, 0xa6e3a1, 0xf38ba8, 0xfab387, 0xf9e2af, 0x74c7ec, 0x7f849c,
                0xf38ba8, 0xa6e3a1, 0x89b4fa, 0xcba6f7, 0x74c7ec, 0xf38ba8, 0xf9e2af, 0xa6e3a1,
            ], &[0x89b4fa, 0xa6e3a1, 0xcba6f7, 0xfab387, 0xf5c2e7, 0x94e2d5, 0xf9e2af, 0xb4befe]),
            "tokyonight" => Theme::from_hex([
                0x1a1b26, 0xc0caf5, 0x565f89, 0x7aa2f7, 0x292e42, 0x7aa2f7, 0x283457, 0xff9e64,
                0x565f89, 0x7aa2f7, 0x9ece6a, 0xf7768e, 0xff9e64, 0xe0af68, 0x7dcfff, 0x565f89,
                0xf7768e, 0x9ece6a, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xf7768e, 0xe0af68, 0x9ece6a,
            ], &[0x7aa2f7, 0x9ece6a, 0xbb9af7, 0xff9e64, 0xf7768e, 0x73daca, 0xe0af68, 0x2ac3de]),
            "gruvbox" => Theme::from_hex([
                0x282828, 0xebdbb2, 0x928374, 0x83a598, 0x3c3836, 0x83a598, 0x504945, 0xfe8019,
                0x928374, 0x83a598, 0xb8bb26, 0xfb4934, 0xfe8019, 0xfabd2f, 0x83a598, 0x928374,
                0xfb4934, 0xb8bb26, 0x83a598, 0xd3869b, 0x8ec07c, 0xfb4934, 0xfabd2f, 0xb8bb26,
            ], &[0x83a598, 0xb8bb26, 0xd3869b, 0xfe8019, 0x8ec07c, 0xfabd2f, 0xfb4934, 0xa89984]),
            _ => return None,
        })
    }

    /// A built-in theme, or `<themes_dir>/<name>.toml`: a `base` theme and
    /// the tokens it changes, in the same form as `[theme.overrides]`.
    pub fn load(name: &str, themes_dir: Option<&Path>) -> Result<(Theme, Vec<String>), String> {
        if let Some(theme) = Theme::builtin(name) {
            return Ok((theme, vec![]));
        }
        let path = themes_dir.map(|d| d.join(format!("{name}.toml")));
        let Some(source) = path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()) else {
            return Err(format!("no theme `{name}` (built in: {})", BUILTIN.join(", ")));
        };
        let mut table: toml::Table =
            toml::from_str(&source).map_err(|e| format!("theme `{name}`: {e}"))?;
        let base = match table.remove("base") {
            Some(toml::Value::String(b)) => b,
            _ => "dark".into(),
        };
        let mut theme = Theme::builtin(&base)
            .ok_or_else(|| format!("theme `{name}`: no base theme `{base}`"))?;
        let problems = theme.apply(&table, &format!("theme `{name}`"));
        Ok((theme, problems))
    }

    /// Applies overrides: `accent = "#7e9cd8"`, `"status.done" = "green"`
    /// or a nested `[status] done = ...`, `label_palette = [...]`. Colours
    /// are `#rrggbb`, a name (`red`, `darkgray`) or an index (`208`).
    /// Returns what it could not apply.
    pub fn apply(&mut self, overrides: &toml::Table, at: &str) -> Vec<String> {
        let mut problems = vec![];
        let mut flat = vec![];
        flatten("", overrides, &mut flat);
        for (name, value) in flat {
            if name == "label_palette" {
                match value
                    .as_array()
                    .map(|a| a.iter().map(|v| v.as_str().map(parse_color)).collect())
                {
                    Some(Some(Ok(colors))) => self.label_palette = colors,
                    _ => problems.push(format!("{at}: label_palette must be a list of colours")),
                }
                continue;
            }
            let Some(slot) = self.token_mut(&name) else {
                problems.push(format!("{at}: unknown token `{name}`"));
                continue;
            };
            match value.as_str().map(parse_color) {
                Some(Ok(color)) => *slot = color,
                Some(Err(e)) => problems.push(format!("{at}: `{name}`: {e}")),
                None => problems.push(format!("{at}: `{name}` must be a colour in quotes")),
            }
        }
        problems
    }

    pub fn status(&self, category: StatusCategory) -> Color {
        match category {
            StatusCategory::Todo => self.status_todo,
            StatusCategory::Doing => self.status_doing,
            StatusCategory::Done => self.status_done,
        }
    }

    pub fn priority(&self, priority: Priority) -> Color {
        match priority {
            Priority::Highest => self.priority_highest,
            Priority::High => self.priority_high,
            Priority::Medium => self.priority_medium,
            Priority::Low => self.priority_low,
            Priority::Lowest => self.priority_lowest,
        }
    }

    pub fn issue_type(&self, kind: IssueKind) -> Color {
        match kind {
            IssueKind::Bug => self.type_bug,
            IssueKind::Story => self.type_story,
            IssueKind::Task | IssueKind::Other => self.type_task,
            IssueKind::Epic => self.type_epic,
            IssueKind::Subtask => self.type_subtask,
        }
    }

    /// FNV-1a: stable across runs and builds, unlike `DefaultHasher`.
    pub fn label(&self, label: &str) -> Color {
        let hash =
            label.bytes().fold(0x811c9dc5u32, |h, b| (h ^ u32::from(b)).wrapping_mul(0x01000193));
        let n = self.label_palette.len().max(1);
        self.label_palette.get(hash as usize % n).copied().unwrap_or(self.accent)
    }
}

fn flatten<'a>(prefix: &str, table: &'a toml::Table, out: &mut Vec<(String, &'a toml::Value)>) {
    for (key, value) in table {
        let name = if prefix.is_empty() { key.clone() } else { format!("{prefix}.{key}") };
        match value.as_table() {
            Some(inner) => flatten(&name, inner, out),
            None => out.push((name, value)),
        }
    }
}

fn parse_color(s: &str) -> Result<Color, String> {
    Color::from_str(s).map_err(|_| format!("`{s}` is not a colour"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_loads() {
        for name in BUILTIN {
            let theme = Theme::builtin(name).unwrap();
            assert_eq!(theme.label_palette.len(), 8, "{name}");
        }
        assert_eq!(Theme::builtin("dark").unwrap().bg, rgb(0x121212));
        assert_eq!(Theme::builtin("dark").unwrap().ok, rgb(0x5fd75f), "the last token");
    }

    #[test]
    fn overrides_set_tokens_flat_or_nested_and_report_mistakes() {
        let mut theme = Theme::builtin("dark").unwrap();
        let overrides: toml::Table = toml::from_str(
            r##"
            accent = "#7e9cd8"
            "status.done" = "green"
            label_palette = ["red", "208"]
            acent = "red"
            muted = "nope"
            fg = 3
            [priority]
            high = "#ff0000"
            "##,
        )
        .unwrap();
        let problems = theme.apply(&overrides, "[theme.overrides]");
        assert_eq!(theme.accent, rgb(0x7e9cd8));
        assert_eq!(theme.status_done, Color::Green);
        assert_eq!(theme.priority_high, rgb(0xff0000));
        assert_eq!(theme.label_palette, [Color::Red, Color::Indexed(208)]);
        assert_eq!(
            problems,
            [
                "[theme.overrides]: unknown token `acent`",
                "[theme.overrides]: `muted`: `nope` is not a colour",
                "[theme.overrides]: `fg` must be a colour in quotes",
            ]
        );
    }

    #[test]
    fn a_theme_file_builds_on_a_base() {
        let dir = std::env::temp_dir().join(format!("kolum-theme-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("mine.toml"), "base = \"gruvbox\"\naccent = \"#010203\"\n")
            .unwrap();
        let (theme, problems) = Theme::load("mine", Some(&dir)).unwrap();
        assert!(problems.is_empty());
        assert_eq!(theme.accent, rgb(0x010203));
        assert_eq!(theme.bg, Theme::builtin("gruvbox").unwrap().bg);
        assert!(Theme::load("missing", Some(&dir)).unwrap_err().starts_with("no theme `missing`"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_label_keeps_its_colour() {
        let theme = Theme::builtin("dark").unwrap();
        assert_eq!(theme.label("backend"), theme.label("backend"));
        assert_ne!(theme.label("backend"), theme.label("frontend"));
    }
}
