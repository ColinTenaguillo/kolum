//! Searching drawn text, as vim's `/` does: where a query occurs in a
//! line, and the line with those places highlighted.

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use std::ops::Range;

/// Where `query` occurs in `text`, as byte ranges that do not overlap.
/// Smartcase: the case counts only when the query has an upper-case letter.
pub fn find(text: &str, query: &str) -> Vec<Range<usize>> {
    let exact = query.chars().any(char::is_uppercase);
    // One char for one, so the ranges stay on `text`'s bytes: a letter
    // whose lower case is longer (İ) folds to its first char.
    let fold = |c: char| if exact { c } else { c.to_lowercase().next().unwrap_or(c) };
    let query: Vec<char> = query.chars().map(fold).collect();
    let chars: Vec<(usize, char)> = text.char_indices().map(|(i, c)| (i, fold(c))).collect();
    let mut found = vec![];
    let mut i = 0;
    while !query.is_empty() && i + query.len() <= chars.len() {
        if chars[i..i + query.len()].iter().map(|&(_, c)| c).eq(query.iter().copied()) {
            let end = chars.get(i + query.len()).map_or(text.len(), |&(at, _)| at);
            found.push(chars[i].0..end);
            i += query.len();
        } else {
            i += 1;
        }
    }
    found
}

/// The line with `ranges` of its text (as [`find`] gives them) drawn in
/// `style` over their own.
pub fn mark(mut line: Line<'static>, ranges: &[Range<usize>], style: Style) -> Line<'static> {
    if ranges.is_empty() {
        return line;
    }
    let mut spans = vec![];
    let mut at = 0;
    for span in std::mem::take(&mut line.spans) {
        let text = span.content.as_ref();
        let end = at + text.len();
        let mut cuts: Vec<usize> = ranges
            .iter()
            .flat_map(|r| [r.start, r.end])
            .filter(|&c| c > at && c < end)
            .chain([at, end])
            .collect();
        cuts.sort_unstable();
        cuts.dedup();
        for piece in cuts.windows(2) {
            let (a, b) = (piece[0], piece[1]);
            let lit = ranges.iter().any(|r| r.start <= a && b <= r.end);
            let style = if lit { span.style.patch(style) } else { span.style };
            spans.push(Span::styled(text[a - at..b - at].to_string(), style));
        }
        at = end;
    }
    line.spans = spans;
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    #[test]
    fn lower_case_finds_any_case_and_upper_case_its_own() {
        assert_eq!(find("TTL, ttl and Ttl", "ttl"), [0..3, 5..8, 13..16]);
        assert_eq!(find("TTL, ttl and Ttl", "Ttl"), vec![13..16]);
        assert_eq!(find("Été, été", "été"), [0..5, 7..12], "bytes, not chars");
        assert_eq!(find("aaaa", "aa"), [0..2, 2..4], "no overlap");
        assert_eq!(find("abc", ""), []);
        assert_eq!(find("ab", "abc"), []);
    }

    #[test]
    fn a_match_across_spans_is_lit_in_each_and_the_rest_keeps_its_style() {
        let red = Style::new().fg(Color::Red);
        let line = Line::from(vec![Span::styled("the T", red), Span::raw("TL is")]);
        let lit = Style::new().bg(Color::Yellow);
        let marked = mark(line, &find("the TTL is", "ttl"), lit);
        let pieces: Vec<(&str, Style)> =
            marked.spans.iter().map(|s| (s.content.as_ref(), s.style)).collect();
        assert_eq!(
            pieces,
            [("the ", red), ("T", red.bg(Color::Yellow)), ("TL", lit), (" is", Style::new())]
        );
    }
}
