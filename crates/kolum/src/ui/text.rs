//! Fitting text into cells: display width, wrapping, truncation.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub fn width(s: &str) -> usize {
    s.width()
}

/// Cuts to `max` cells, ending in `…` when anything was cut.
pub fn truncate(s: &str, max: usize) -> String {
    if s.width() <= max {
        return s.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let w = c.width().unwrap_or(0);
        if used + w + 1 > max {
            break;
        }
        out.push(c);
        used += w;
    }
    if max > 0 {
        out.push('…');
    }
    out
}

/// Word-wraps into at most `max_lines`, the last one truncated if the text
/// goes on. A word longer than a line is cut.
pub fn wrap(s: &str, cols: usize, max_lines: usize) -> Vec<String> {
    let mut lines: Vec<String> = vec![];
    let mut line = String::new();
    for word in s.split_whitespace() {
        if line.is_empty() {
            line = word.to_string();
        } else if line.width() + 1 + word.width() <= cols {
            line.push(' ');
            line.push_str(word);
        } else {
            lines.push(std::mem::replace(&mut line, word.to_string()));
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    if max_lines == 0 {
        return vec![];
    }
    if lines.len() > max_lines {
        // Words are left over: fold them into the last line, whose
        // truncation then shows that the text goes on.
        let rest = lines.split_off(max_lines).join(" ");
        if let Some(last) = lines.last_mut() {
            last.push(' ');
            last.push_str(&rest);
        }
    }
    lines.iter().map(|l| truncate(l, cols)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_marks_the_cut() {
        assert_eq!(truncate("hello", 5), "hello");
        assert_eq!(truncate("hello world", 8), "hello w…");
        assert_eq!(truncate("x", 0), "");
    }

    #[test]
    fn wrap_breaks_on_words_and_ellipsizes_the_rest() {
        assert_eq!(
            wrap("Login times out after SSO redirect", 14, 2),
            ["Login times", "out after SSO…"]
        );
        assert_eq!(wrap("short", 14, 2), ["short"]);
        assert_eq!(wrap("a verylongwordthatdoesnotfit", 8, 2), ["a", "verylon…"]);
        assert!(wrap("", 10, 2).is_empty());
    }
}
