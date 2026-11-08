//! Renders the domain's RichText. The only renderer: every provider's
//! format reaches the screen through this.

use super::text::{width, wrap};
use crate::theme::Theme;
use kolum_domain::RichText;
use kolum_domain::rich_text::{Block, Inline, Marks, PanelKind, inline_text};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

pub fn lines(rt: &RichText, theme: &Theme, cols: u16) -> Vec<Line<'static>> {
    let mut out = vec![];
    for (i, block) in rt.blocks.iter().enumerate() {
        if i > 0 {
            out.push(Line::default());
        }
        render_block(block, theme, cols as usize, &mut out);
    }
    out
}

/// Prefixes each line with `first` then `rest`: list bullets, quote and
/// panel gutters.
fn indent(
    lines: Vec<Line<'static>>,
    first: Span<'static>,
    rest: Span<'static>,
) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .enumerate()
        .map(|(i, line)| {
            let mut spans = vec![if i == 0 { first.clone() } else { rest.clone() }];
            spans.extend(line.spans);
            Line::from(spans).style(line.style)
        })
        .collect()
}

fn render_block(block: &Block, theme: &Theme, cols: usize, out: &mut Vec<Line<'static>>) {
    match block {
        Block::Paragraph(inlines) => out.extend(inline_lines(inlines, theme, Style::default())),
        Block::Heading { level, content } => {
            out.extend(inline_lines(content, theme, heading_style(*level, theme)))
        }
        Block::List { ordered, items } => list(*ordered, items, theme, cols, out),
        Block::Code { language, text } => code(language.as_deref(), text, theme, out),
        Block::Quote(blocks) => quote(blocks, theme, cols, out),
        Block::Panel { kind, blocks } => callout(*kind, blocks, theme, cols, out),
        Block::Table { header, rows } => out.extend(table(rows, *header, theme, cols)),
        Block::Rule => out.push(Line::from(Span::styled(
            "─".repeat(cols.min(80)),
            Style::new().fg(theme.border),
        ))),
    }
}

/// Each item's blocks hung under its bullet or number.
fn list(
    ordered: bool,
    items: &[Vec<Block>],
    theme: &Theme,
    cols: usize,
    out: &mut Vec<Line<'static>>,
) {
    for (n, item) in items.iter().enumerate() {
        let bullet = if ordered { format!("{}. ", n + 1) } else { "• ".to_string() };
        let pad = " ".repeat(width(&bullet));
        let mut inner = vec![];
        for block in item {
            render_block(block, theme, cols.saturating_sub(pad.len()), &mut inner);
        }
        let bullet = Span::styled(bullet, Style::new().fg(theme.muted));
        out.extend(indent(inner, bullet, Span::raw(pad)));
    }
}

/// Behind a gutter, under its language when it names one.
fn code(language: Option<&str>, text: &str, theme: &Theme, out: &mut Vec<Line<'static>>) {
    let gutter = Span::styled("│ ", Style::new().fg(theme.border));
    if let Some(lang) = language {
        out.push(Line::from(Span::styled(format!("╭ {lang}"), Style::new().fg(theme.muted))));
    }
    for line in text.lines() {
        let code = Span::styled(line.to_string(), Style::new().fg(theme.accent));
        out.push(Line::from(vec![gutter.clone(), code]));
    }
}

fn quote(blocks: &[Block], theme: &Theme, cols: usize, out: &mut Vec<Line<'static>>) {
    let mut inner = vec![];
    blocks.iter().for_each(|b| render_block(b, theme, cols.saturating_sub(2), &mut inner));
    let gutter = Span::styled("│ ", Style::new().fg(theme.muted));
    let quoted = indent(inner, gutter.clone(), gutter);
    out.extend(quoted.into_iter().map(|l| l.style(Style::new().fg(theme.muted))));
}

/// A panel, as GitHub draws its alerts: a coloured bar and a title.
fn callout(
    kind: PanelKind,
    blocks: &[Block],
    theme: &Theme,
    cols: usize,
    out: &mut Vec<Line<'static>>,
) {
    let (color, title) = panel(kind, theme);
    let gutter = Span::styled("▌ ", Style::new().fg(color));
    let mut inner =
        vec![Line::from(Span::styled(title, Style::new().fg(color).add_modifier(Modifier::BOLD)))];
    blocks.iter().for_each(|b| render_block(b, theme, cols.saturating_sub(2), &mut inner));
    out.extend(indent(inner, gutter.clone(), gutter));
}

fn heading_style(level: u8, theme: &Theme) -> Style {
    match level {
        1 => Style::new().fg(theme.accent).add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        2 => Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
        _ => Style::new().fg(theme.fg).add_modifier(Modifier::BOLD),
    }
}

/// A panel's colour and the word heading it, as GitHub names its alerts.
fn panel(kind: PanelKind, theme: &Theme) -> (ratatui::style::Color, &'static str) {
    match kind {
        PanelKind::Info => (theme.accent, "Important"),
        PanelKind::Note => (theme.accent, "Note"),
        PanelKind::Success => (theme.ok, "Tip"),
        PanelKind::Warning => (theme.warn, "Warning"),
        PanelKind::Error => (theme.error, "Caution"),
    }
}

/// The columns' widths within `room` cells: each its widest cell when all
/// fit; otherwise those narrower than a fair share keep theirs and the
/// others share what is left.
fn fit_widths(natural: &[usize], room: usize) -> Vec<usize> {
    if natural.iter().sum::<usize>() <= room {
        return natural.to_vec();
    }
    let mut kept = vec![false; natural.len()];
    loop {
        let (used, left) = natural
            .iter()
            .zip(&kept)
            .fold((0, 0), |(u, l), (w, k)| if *k { (u + w, l) } else { (u, l + 1) });
        let share = room.saturating_sub(used) / left.max(1);
        let fits: Vec<usize> =
            (0..natural.len()).filter(|&c| !kept[c] && natural[c] <= share).collect();
        if fits.is_empty() {
            let width = |c: usize| if kept[c] { natural[c] } else { share.max(1) };
            return (0..natural.len()).map(width).collect();
        }
        fits.into_iter().for_each(|c| kept[c] = true);
    }
}

/// Words wider than `cols` broken into pieces that fit, so that wrapping
/// moves them on rather than cutting them short.
fn split_long_words(text: &str, cols: usize) -> String {
    let mut out = String::new();
    for word in text.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        let mut used = 0;
        for c in word.chars() {
            let w = width(c.encode_utf8(&mut [0; 4]));
            if used > 0 && used + w > cols {
                out.push(' ');
                used = 0;
            }
            out.push(c);
            used += w;
        }
    }
    out
}

/// Each cell wrapped within its column, a row as tall as its tallest cell.
fn table(
    rows: &[Vec<Vec<Inline>>],
    header: bool,
    theme: &Theme,
    cols: usize,
) -> Vec<Line<'static>> {
    let cells: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|cell| {
                    let mut s = String::new();
                    inline_text(cell, &mut s);
                    s
                })
                .collect()
        })
        .collect();
    let ncols = cells.iter().map(Vec::len).max().unwrap_or_default();
    let natural: Vec<usize> = (0..ncols)
        .map(|c| cells.iter().filter_map(|r| r.get(c)).map(|s| width(s)).max().unwrap_or_default())
        .collect();
    let widths = fit_widths(&natural, cols.saturating_sub(3 * ncols.saturating_sub(1)));
    let sep = Style::new().fg(theme.border);
    let mut out = vec![];
    for (r, row) in cells.iter().enumerate() {
        let is_header = header && r == 0;
        let style =
            if is_header { Style::new().add_modifier(Modifier::BOLD) } else { Style::default() };
        let wrapped: Vec<Vec<String>> = widths
            .iter()
            .enumerate()
            .map(|(c, w)| {
                let text = split_long_words(row.get(c).map(String::as_str).unwrap_or(""), *w);
                wrap(&text, *w, usize::MAX)
            })
            .collect();
        // An empty row keeps a line.
        let height = wrapped.iter().map(Vec::len).max().unwrap_or_default().max(1);
        for n in 0..height {
            let mut spans = vec![];
            for (c, w) in widths.iter().enumerate() {
                if c > 0 {
                    spans.push(Span::styled(" │ ", sep));
                }
                let text = wrapped[c].get(n).map(String::as_str).unwrap_or("");
                let pad = " ".repeat(w.saturating_sub(width(text)));
                spans.push(Span::styled(format!("{text}{pad}"), style));
            }
            out.push(Line::from(spans));
        }
        if is_header {
            let rule: Vec<String> = widths.iter().map(|w| "─".repeat(*w)).collect();
            out.push(Line::from(Span::styled(rule.join("─┼─"), sep)));
        }
    }
    out
}

fn mark_style(marks: Marks, theme: &Theme) -> Style {
    let mut style = Style::default();
    if marks.bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    if marks.italic {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if marks.strike {
        style = style.add_modifier(Modifier::CROSSED_OUT);
    }
    if marks.code {
        style = style.fg(theme.accent);
    }
    style
}

/// Inline runs as lines: a hard break starts a new one. Wrapping is left
/// to the Paragraph that shows them.
pub fn inline_lines(inlines: &[Inline], theme: &Theme, base: Style) -> Vec<Line<'static>> {
    let mut lines = vec![];
    let mut spans: Vec<Span<'static>> = vec![];
    for inline in inlines {
        let span = match inline {
            Inline::Text { text, marks } => {
                Span::styled(text.clone(), base.patch(mark_style(*marks, theme)))
            }
            Inline::Link { text, .. } => {
                Span::styled(text.clone(), base.fg(theme.accent).add_modifier(Modifier::UNDERLINED))
            }
            Inline::Mention { name, .. } => {
                Span::styled(format!("@{name}"), base.fg(theme.accent).add_modifier(Modifier::BOLD))
            }
            Inline::Emoji(e) => Span::styled(e.clone(), base),
            Inline::Break => {
                lines.push(Line::from(std::mem::take(&mut spans)));
                continue;
            }
        };
        spans.push(span);
    }
    lines.push(Line::from(spans));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(lines: &[Line]) -> Vec<String> {
        lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect()).collect()
    }

    #[test]
    fn columns_keep_their_width_when_it_fits_and_share_the_rest_when_not() {
        assert_eq!(fit_widths(&[2, 10, 30], 60), [2, 10, 30], "room for all");
        assert_eq!(fit_widths(&[2, 40, 60], 50), [2, 24, 24], "the narrow one keeps its own");
        assert_eq!(fit_widths(&[2, 10, 60], 40), [2, 10, 28], "the rest to the widest");
        assert_eq!(fit_widths(&[30, 30], 4), [2, 2], "never under a little");
        assert_eq!(fit_widths(&[30, 30], 1), [1, 1], "one cell at least");
        assert_eq!(fit_widths(&[10, 10], 20), [10, 10], "exactly the room");
        assert_eq!(fit_widths(&[15, 30], 31), [15, 16], "a share's width keeps its own");
    }

    #[test]
    fn a_wide_table_wraps_its_cells_within_the_width() {
        let theme = Theme::builtin("dark").unwrap();
        let rt = RichText::from_markdown(
            "| # | Test | Fails when |\n|---|---|---|\n| 1 | Effective date consistency | benefit_at is not a date, or its month differs |",
        );
        let shown = plain(&lines(&rt, &theme, 40));
        assert!(shown.iter().all(|l| width(l) <= 40), "{shown:#?}");
        assert_eq!(shown[0].trim_end(), "# │ Test             │ Fails when", "{shown:#?}");
        assert!(shown[2].starts_with("1 │ Effective date   │ benefit_at is"), "{shown:#?}");
        assert!(shown[3].starts_with("  │ consistency      │ not a date, or"), "{shown:#?}");
        assert_eq!(
            shown[5].trim_end(),
            "  │                  │ differs",
            "a row as tall as its tallest cell"
        );
    }

    #[test]
    fn long_words_are_split_at_the_width_and_a_wide_character_is_not_pushed_off() {
        assert_eq!(split_long_words("abcdef gh", 2), "ab cd ef gh");
        assert_eq!(split_long_words("abc", 1), "a b c");
        assert_eq!(split_long_words("漢字", 1), "漢 字", "a character wider than the column");
    }

    #[test]
    fn a_level_one_heading_is_underlined_in_the_accent() {
        let theme = Theme::builtin("dark").unwrap();
        let lines = lines(&RichText::from_markdown("# One\n\n## Two"), &theme, 40);
        let style = |text: &str| {
            let line = lines.iter().find(|l| l.to_string().contains(text)).unwrap();
            line.spans.iter().find(|s| s.content.contains(text)).unwrap().style
        };
        assert_eq!(style("One").fg, Some(theme.accent));
        assert!(style("One").add_modifier.contains(Modifier::UNDERLINED));
        assert!(!style("Two").add_modifier.contains(Modifier::UNDERLINED));
    }

    #[test]
    fn an_empty_table_row_keeps_its_line() {
        let theme = Theme::builtin("dark").unwrap();
        let rows = vec![vec![vec![Inline::text("a")]], vec![vec![]], vec![vec![Inline::text("b")]]];
        let drawn = plain(&table(&rows, false, &theme, 20));
        assert_eq!(drawn.len(), 3, "{drawn:?}");
    }

    #[test]
    fn a_word_wider_than_its_column_is_split_not_cut() {
        let theme = Theme::builtin("dark").unwrap();
        let rt = RichText::from_markdown(
            "| a | b |\n|---|---|\n| EXCLUDED_PENSION_ID_COUNT | the pensions failing a test |",
        );
        let shown = plain(&lines(&rt, &theme, 24));
        let first: String =
            shown[2..].iter().map(|l| l.split(" │ ").next().unwrap().trim()).collect();
        assert_eq!(first, "EXCLUDED_PENSION_ID_COUNT", "{shown:#?}");
    }

    #[test]
    fn every_block_kind_has_a_shape() {
        let theme = Theme::builtin("dark").unwrap();
        let rt = RichText::from_markdown(
            "# Title\n\n- one\n- two\n  1. nested\n\n```rust\nlet x = 1;\n```\n\n> [!NOTE]\n> careful\n\n\
             | Plan | Rate |\n|---|---|\n| Free | 60 |\n\nby [@Ana](mention:u1)",
        );
        assert_eq!(
            plain(&lines(&rt, &theme, 40)),
            [
                "Title",
                "",
                "• one",
                "• two",
                "  1. nested",
                "",
                "╭ rust",
                "│ let x = 1;",
                "",
                "▌ Note",
                "▌ careful",
                "",
                "Plan │ Rate",
                "─────┼─────",
                "Free │ 60  ",
                "",
                "by @Ana",
            ]
        );
    }

    #[test]
    fn quotes_panels_and_deep_headings() {
        let theme = Theme::builtin("dark").unwrap();
        let rt = RichText::from_markdown(
            "### Deep\n\n> said\n\n> [!TIP]\n> a\n\n> [!IMPORTANT]\n> b\n\n> [!CAUTION]\n> c\n\n> [!WARNING]\n> d\n\n---\n\n~~old~~ *it* a  \nb",
        );
        let shown = plain(&lines(&rt, &theme, 10));
        assert_eq!(
            shown,
            [
                "Deep",
                "",
                "│ said",
                "",
                "▌ Tip",
                "▌ a",
                "",
                "▌ Important",
                "▌ b",
                "",
                "▌ Caution",
                "▌ c",
                "",
                "▌ Warning",
                "▌ d",
                "",
                "──────────",
                "",
                "old it a",
                "b",
            ]
        );
        assert_eq!(heading_style(3, &theme).fg, Some(theme.fg));
        assert_eq!(panel(PanelKind::Error, &theme).0, theme.error);
    }
}
