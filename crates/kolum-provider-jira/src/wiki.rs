//! Jira's wiki markup into `RichText`: Data Center's format. The common
//! part of it; what it has no place for marks the text lossy: a colour or an
//! underline keeps its text, an image goes.

use kolum_domain::UserId;
use kolum_domain::rich_text::{Block, Inline, Marks, PanelKind, RichText};

pub fn to_rich_text(text: &str) -> RichText {
    let mut reader = Reader { lossy: false };
    let lines: Vec<&str> = text.lines().collect();
    let blocks = reader.blocks(&lines);
    RichText { blocks, lossy: reader.lossy }
}

/// The macros that hold blocks, closed by the same tag: `{code}…{code}`.
const BLOCK_MACROS: [&str; 8] =
    ["code", "noformat", "quote", "panel", "info", "note", "warning", "tip"];

struct Reader {
    lossy: bool,
}

impl Reader {
    fn blocks(&mut self, lines: &[&str]) -> Vec<Block> {
        let mut out = vec![];
        let mut para: Vec<&str> = vec![];
        let mut i = 0;
        while i < lines.len() {
            let line = lines[i].trim();
            let found = self.block_at(&lines[i..]);
            if (found.is_some() || line.is_empty()) && !para.is_empty() {
                out.push(self.paragraph(&para));
                para.clear();
            }
            match found {
                Some((blocks, taken)) => {
                    out.extend(blocks);
                    i += taken;
                }
                None => {
                    if !line.is_empty() {
                        para.push(line);
                    }
                    i += 1;
                }
            }
        }
        if !para.is_empty() {
            out.push(self.paragraph(&para));
        }
        out
    }

    /// The blocks the first line opens, if it opens any, and how many lines
    /// they take: a macro, then what follows its closing tag on its line.
    fn block_at(&mut self, lines: &[&str]) -> Option<(Vec<Block>, usize)> {
        let line = lines[0].trim();
        let run = |pred: &dyn Fn(&str) -> bool| lines.iter().take_while(|l| pred(l.trim())).count();
        if let Some((tag, params, rest)) = block_macro(line) {
            let (body, next, after) = macro_body(tag, rest, &lines[1..]);
            let mut blocks = vec![self.macro_block(tag, params, &body)];
            blocks.extend(self.blocks(&[after.as_str()]));
            return Some((blocks, 1 + next));
        }
        if let Some((level, rest)) = heading(line) {
            return Some((vec![Block::Heading { level, content: self.inlines(rest) }], 1));
        }
        if let Some(rest) = line.strip_prefix("bq. ") {
            return Some((vec![Block::Quote(vec![Block::Paragraph(self.inlines(rest))])], 1));
        }
        if line == "----" {
            return Some((vec![Block::Rule], 1));
        }
        if line.starts_with('|') {
            let n = run(&|l| l.starts_with('|'));
            return Some((vec![self.table(&lines[..n])], n));
        }
        let kind = list_marker(line)?.0.as_bytes()[0];
        let n = run(&|l| list_marker(l).is_some_and(|(m, _)| m.as_bytes()[0] == kind));
        let entries: Vec<_> = lines[..n].iter().filter_map(|l| list_marker(l.trim())).collect();
        Some((vec![self.list(&entries, 0)], n))
    }

    /// A line break in wiki markup is one in the text.
    fn paragraph(&mut self, lines: &[&str]) -> Block {
        let mut inlines = vec![];
        for (n, line) in lines.iter().enumerate() {
            if n > 0 {
                inlines.push(Inline::Break);
            }
            inlines.extend(self.inlines(line));
        }
        Block::Paragraph(inlines)
    }

    fn macro_block(&mut self, tag: &str, params: &str, body: &str) -> Block {
        let lines: Vec<&str> = body.lines().collect();
        match tag {
            "code" | "noformat" => Block::Code {
                language: (tag == "code").then(|| code_language(params)).flatten(),
                text: body.trim_matches('\n').to_string(),
            },
            "quote" => Block::Quote(self.blocks(&lines)),
            _ => Block::Panel { kind: panel_kind(tag), blocks: self.blocks(&lines) },
        }
    }

    /// `* a`, `** nested`, `# numbered`: the depth is the marker's length,
    /// the kind its character at that depth.
    fn list(&mut self, entries: &[(&str, &str)], depth: usize) -> Block {
        let ordered = entries[0].0.as_bytes().get(depth) == Some(&b'#');
        let mut items: Vec<Vec<Block>> = vec![];
        let mut i = 0;
        while i < entries.len() {
            let (marker, text) = entries[i];
            if marker.len() <= depth + 1 {
                items.push(vec![Block::Paragraph(self.inlines(text))]);
                i += 1;
                continue;
            }
            let n = entries[i..].iter().take_while(|(m, _)| m.len() > depth + 1).count();
            let nested = self.list(&entries[i..i + n], depth + 1);
            match items.last_mut() {
                Some(item) => item.push(nested),
                None => items.push(vec![nested]),
            }
            i += n;
        }
        Block::List { ordered, items }
    }

    /// `||head||head||` then `|cell|cell|`; a `|` inside a link or code is
    /// not a cell's end.
    fn table(&mut self, lines: &[&str]) -> Block {
        let header = lines[0].trim().starts_with("||");
        let rows = lines
            .iter()
            .map(|l| split_cells(l.trim()).into_iter().map(|c| self.inlines(c.trim())).collect())
            .collect();
        Block::Table { header, rows }
    }

    fn inlines(&mut self, s: &str) -> Vec<Inline> {
        let mut out = vec![];
        self.spans(s, Marks::default(), &mut out);
        merge(out)
    }

    fn spans(&mut self, s: &str, marks: Marks, out: &mut Vec<Inline>) {
        let mut i = 0;
        while let Some(c) = s[i..].chars().next() {
            match self.token(s, i, marks) {
                Some((inlines, taken)) => {
                    out.extend(inlines);
                    i += taken;
                }
                None => {
                    out.push(Inline::Text { text: c.into(), marks });
                    i += c.len_utf8();
                }
            }
        }
    }

    /// What starts at `i` when it is more than a character of text, and
    /// its length.
    fn token(&mut self, s: &str, i: usize, marks: Marks) -> Option<(Vec<Inline>, usize)> {
        let rest = &s[i..];
        if let Some(found) = escape(rest, marks).or_else(|| code(rest, marks)) {
            return Some(found);
        }
        if let Some(taken) = dropped(rest) {
            self.lossy = true;
            return Some((vec![], taken));
        }
        if let Some(found) = bracket(rest, marks) {
            return Some(found);
        }
        let c = rest.chars().next()?;
        let end = closing(s, i, c)?;
        // An underline has no mark here: its text stays, plain.
        self.lossy |= c == '+';
        let mut inner = vec![];
        self.spans(&s[i + 1..end], mark(c, marks), &mut inner);
        Some((inner, end + 1 - i))
    }
}

/// `\\` is a line break, `\*` a star.
fn escape(rest: &str, marks: Marks) -> Option<(Vec<Inline>, usize)> {
    let after = rest.strip_prefix('\\')?;
    if after.starts_with('\\') {
        return Some((vec![Inline::Break], 2));
    }
    let c = after.chars().next()?;
    Some((vec![Inline::Text { text: c.into(), marks }], 1 + c.len_utf8()))
}

/// `{{monospace}}`.
fn code(rest: &str, marks: Marks) -> Option<(Vec<Inline>, usize)> {
    let end = rest.strip_prefix("{{")?.find("}}")?;
    let text = rest[2..2 + end].to_string();
    Some((vec![Inline::Text { text, marks: Marks { code: true, ..marks } }], end + 4))
}

/// What has no place here and goes, its text kept: a colour's tags, an
/// image. Its length.
fn dropped(rest: &str) -> Option<usize> {
    let colour = rest.starts_with("{color").then(|| rest.find('}')).flatten();
    colour.map(|end| end + 1).or_else(|| image(rest))
}

/// `[…]`: a link or a mention, or the brackets as text.
fn bracket(rest: &str, marks: Marks) -> Option<(Vec<Inline>, usize)> {
    let end = rest.strip_prefix('[')?.find(']')?;
    let inline = link(&rest[1..1 + end])
        .unwrap_or_else(|| Inline::Text { text: rest[..end + 2].into(), marks });
    Some((vec![inline], end + 2))
}

fn mark(c: char, marks: Marks) -> Marks {
    match c {
        '*' => Marks { bold: true, ..marks },
        '_' => Marks { italic: true, ..marks },
        '-' => Marks { strike: true, ..marks },
        _ => marks,
    }
}

/// `{code:java}`, `{code:language=java|title=x}`: the language, if named.
fn code_language(params: &str) -> Option<String> {
    let first = params.split('|').next()?;
    let language = first.strip_prefix("language=").unwrap_or(first);
    (!language.is_empty() && !language.contains('=')).then(|| language.to_string())
}

fn panel_kind(tag: &str) -> PanelKind {
    match tag {
        "note" => PanelKind::Note,
        "warning" => PanelKind::Warning,
        "tip" => PanelKind::Success,
        _ => PanelKind::Info,
    }
}

/// `{tag}` or `{tag:params}` opening a line, and what follows it there.
fn block_macro(line: &str) -> Option<(&'static str, &str, &str)> {
    let open = line.strip_prefix('{')?;
    let tag = BLOCK_MACROS
        .into_iter()
        .find(|t| open.strip_prefix(t).is_some_and(|r| r.starts_with('}') || r.starts_with(':')))?;
    let close = open.find('}')?;
    let params = open[tag.len()..close].trim_start_matches(':');
    Some((tag, params, &open[close + 1..]))
}

/// The body of a block macro up to its closing tag, how many of the
/// following lines it took, and what follows the tag on its line.
fn macro_body(tag: &str, first: &str, lines: &[&str]) -> (String, usize, String) {
    let close = format!("{{{tag}}}");
    let after = |line: &str, end: usize| line[end + close.len()..].to_string();
    if let Some(end) = first.find(&close) {
        return (first[..end].to_string(), 0, after(first, end));
    }
    let mut body = first.to_string();
    for (n, line) in lines.iter().enumerate() {
        body.push('\n');
        if let Some(end) = line.find(&close) {
            body.push_str(&line[..end]);
            return (body, n + 1, after(line, end));
        }
        body.push_str(line);
    }
    (body, lines.len(), String::new())
}

fn heading(line: &str) -> Option<(u8, &str)> {
    let rest = line.strip_prefix('h')?;
    let level = rest.chars().next()?.to_digit(10).filter(|l| (1..=6).contains(l))?;
    Some((level as u8, rest[1..].strip_prefix(". ")?))
}

/// `* `, `## `, `#* `, `- `: the marker and the item's text.
fn list_marker(line: &str) -> Option<(&str, &str)> {
    let len = line.find(|c| !matches!(c, '*' | '#' | '-'))?;
    let (marker, rest) = line.split_at(len);
    let fits = !marker.is_empty() && (marker == "-" || !marker.contains('-'));
    fits.then_some((marker, rest.strip_prefix(' ')?))
}

/// The cells of a table row: split on `|` or `||` outside `[…]` and `{…}`;
/// an empty cell between two keeps its place.
fn split_cells(row: &str) -> Vec<&str> {
    let (mut cells, mut depth, mut start) = (vec![], 0i32, 0);
    let mut chars = row.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        match c {
            '[' | '{' => depth += 1,
            ']' | '}' => depth -= 1,
            '|' if depth <= 0 => {
                cells.push(&row[start..i]);
                start = i + 1;
                if chars.next_if(|&(_, next)| next == '|').is_some() {
                    start += 1;
                }
            }
            _ => {}
        }
    }
    cells.push(&row[start..]);
    // What is before the first separator and after the last is no cell.
    if cells.first().is_some_and(|c| c.trim().is_empty()) {
        cells.remove(0);
    }
    if cells.last().is_some_and(|c| c.trim().is_empty()) {
        cells.pop();
    }
    cells
}

/// `[text|url]`, `[url]`, `[~accountid:id]`; `None` for brackets that only
/// hold words, which are text.
fn link(inside: &str) -> Option<Inline> {
    if let Some(user) = inside.strip_prefix('~') {
        let id = user.strip_prefix("accountid:").unwrap_or(user);
        return Some(Inline::Mention { id: Some(UserId::new(id)), name: id.to_string() });
    }
    let mut parts = inside.split('|');
    let first = parts.next()?;
    let (text, url) = match parts.next() {
        Some(url) => (first, url),
        None => (first, first),
    };
    let is_url = url.contains("://") || url.starts_with("mailto:");
    is_url.then(|| Inline::Link { text: text.trim().into(), url: url.trim().into() })
}

/// `!picture.png!` or `!picture.png|thumbnail!`: its length, if it is one.
/// A name without an extension is words between exclamation marks.
fn image(rest: &str) -> Option<usize> {
    let inner = rest.strip_prefix('!')?;
    let end = inner.find(|c: char| c == '!' || c.is_whitespace())?;
    let name = inner[..end].split('|').next().unwrap_or_default();
    (name.contains('.') && inner[end..].starts_with('!')).then_some(end + 2)
}

/// Where the mark `c` opened at `i` closes: it opens after a non-word
/// character and before a non-space, and closes after a non-space and
/// before a non-word character, so `snake_case` and `well-known` stay text.
fn closing(s: &str, i: usize, c: char) -> Option<usize> {
    if !matches!(c, '*' | '_' | '-' | '+') {
        return None;
    }
    let word = |ch: Option<char>| ch.is_some_and(char::is_alphanumeric);
    let space = |ch: Option<char>| ch.is_none_or(char::is_whitespace);
    if word(s[..i].chars().next_back()) || space(s[i + 1..].chars().next()) {
        return None;
    }
    s[i + 1..].match_indices(c).map(|(j, _)| i + 1 + j).find(|&j| {
        j > i + 1 && !space(s[..j].chars().next_back()) && !word(s[j + 1..].chars().next())
    })
}

/// Adjacent runs of text with the same marks, as one.
fn merge(inlines: Vec<Inline>) -> Vec<Inline> {
    let mut out: Vec<Inline> = vec![];
    for inline in inlines {
        if let (Some(Inline::Text { text: last, marks: m }), Inline::Text { text, marks }) =
            (out.last_mut(), &inline)
            && m == marks
        {
            last.push_str(text);
            continue;
        }
        out.push(inline);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use kolum_domain::UserId;
    use kolum_domain::rich_text::{Marks, PanelKind};

    fn text(s: &str) -> Inline {
        Inline::text(s)
    }

    fn marked(s: &str, marks: Marks) -> Inline {
        Inline::Text { text: s.into(), marks }
    }

    fn code(s: &str) -> Inline {
        marked(s, Marks { code: true, ..Marks::default() })
    }

    fn para(s: &str) -> Block {
        Block::Paragraph(vec![text(s)])
    }

    #[test]
    fn headings_and_paragraphs_keep_their_line_breaks() {
        let rt = to_rich_text("h3. Context\nIC-1 added {{a_b}} to it.\nSecond line\n\nNext");
        assert_eq!(
            rt.blocks,
            [
                Block::Heading { level: 3, content: vec![text("Context")] },
                Block::Paragraph(vec![
                    text("IC-1 added "),
                    code("a_b"),
                    text(" to it."),
                    Inline::Break,
                    text("Second line"),
                ]),
                para("Next"),
            ]
        );
        assert!(!rt.lossy);
    }

    #[test]
    fn lists_nest_by_their_markers_and_a_new_marker_starts_a_new_list() {
        let rt = to_rich_text("For each:\n* a\n** b\n* c\n# one\n# two\n- dash");
        assert_eq!(
            rt.blocks,
            [
                para("For each:"),
                Block::List {
                    ordered: false,
                    items: vec![
                        vec![
                            para("a"),
                            Block::List { ordered: false, items: vec![vec![para("b")]] }
                        ],
                        vec![para("c")],
                    ]
                },
                Block::List { ordered: true, items: vec![vec![para("one")], vec![para("two")]] },
                Block::List { ordered: false, items: vec![vec![para("dash")]] },
            ]
        );
    }

    #[test]
    fn marks_open_and_close_at_word_boundaries_only() {
        let rt =
            to_rich_text("*bold* _it_ -gone- *_both_* snake_case_name a - b well-known x*y -2w");
        let bold = Marks { bold: true, ..Marks::default() };
        assert_eq!(
            rt.blocks,
            [Block::Paragraph(vec![
                marked("bold", bold),
                text(" "),
                marked("it", Marks { italic: true, ..Marks::default() }),
                text(" "),
                marked("gone", Marks { strike: true, ..Marks::default() }),
                text(" "),
                marked("both", Marks { italic: true, ..bold }),
                text(" snake_case_name a - b well-known x*y -2w"),
            ])]
        );
    }

    #[test]
    fn escapes_and_forced_line_breaks() {
        let rt = to_rich_text(r"\*not bold\* one\\two");
        assert_eq!(
            rt.blocks,
            [Block::Paragraph(vec![text("*not bold* one"), Inline::Break, text("two")])]
        );
    }

    #[test]
    fn links_mentions_and_plain_brackets() {
        let rt = to_rich_text("[Docs|https://x.io] [https://y.io] [~accountid:abc] [WIP]");
        assert_eq!(
            rt.blocks,
            [Block::Paragraph(vec![
                Inline::Link { text: "Docs".into(), url: "https://x.io".into() },
                text(" "),
                Inline::Link { text: "https://y.io".into(), url: "https://y.io".into() },
                text(" "),
                Inline::Mention { id: Some(UserId::new("abc")), name: "abc".into() },
                text(" [WIP]"),
            ])]
        );
    }

    #[test]
    fn code_and_noformat_keep_their_text_as_it_is() {
        let rt = to_rich_text("{code:java}\nint *x* = 1;\n{code}\n{noformat}a\nb{noformat}");
        assert_eq!(
            rt.blocks,
            [
                Block::Code { language: Some("java".into()), text: "int *x* = 1;".into() },
                Block::Code { language: None, text: "a\nb".into() },
            ]
        );
    }

    #[test]
    fn a_code_block_names_its_language_by_its_first_parameter_or_language() {
        let named = to_rich_text("{code:language=python|title=x}print(){code}");
        let untitled = to_rich_text("{code:title=x}a{code}");
        assert_eq!(
            named.blocks,
            [Block::Code { language: Some("python".into()), text: "print()".into() }]
        );
        assert_eq!(untitled.blocks, [Block::Code { language: None, text: "a".into() }]);
    }

    #[test]
    fn quotes_and_panels_hold_blocks() {
        let rt = to_rich_text(
            "bq. said\n{quote}\nq1\n{quote}\n{info}\nheads up\n{info}\n{warning:title=x}w{warning}\n{tip}t{tip}",
        );
        assert_eq!(
            rt.blocks,
            [
                Block::Quote(vec![para("said")]),
                Block::Quote(vec![para("q1")]),
                Block::Panel { kind: PanelKind::Info, blocks: vec![para("heads up")] },
                Block::Panel { kind: PanelKind::Warning, blocks: vec![para("w")] },
                Block::Panel { kind: PanelKind::Success, blocks: vec![para("t")] },
            ]
        );
    }

    #[test]
    fn tables_rules_and_a_link_inside_a_cell() {
        let rt = to_rich_text("||h1||h2||\n|a|[l|https://u.io]|\n----");
        assert_eq!(
            rt.blocks,
            [
                Block::Table {
                    header: true,
                    rows: vec![
                        vec![vec![text("h1")], vec![text("h2")]],
                        vec![
                            vec![text("a")],
                            vec![Inline::Link { text: "l".into(), url: "https://u.io".into() }]
                        ],
                    ]
                },
                Block::Rule,
            ]
        );
    }

    #[test]
    fn what_has_no_place_keeps_its_text_and_marks_the_text_lossy() {
        let rt = to_rich_text("see !shot.png|thumbnail! and {color:red}red{color}");
        assert_eq!(rt.blocks, [para("see  and red")]);
        assert!(rt.lossy);
    }

    #[test]
    fn an_underline_keeps_its_text_and_marks_the_text_lossy() {
        let rt = to_rich_text("please +do not+ merge");
        assert_eq!(rt.blocks, [para("please do not merge")]);
        assert!(rt.lossy);
    }

    #[test]
    fn exclamation_marks_around_a_word_are_no_picture() {
        let rt = to_rich_text("Done!Really! and !wow!");
        assert_eq!(rt.blocks, [para("Done!Really! and !wow!")]);
        assert!(!rt.lossy);
    }

    #[test]
    fn an_empty_cell_keeps_its_place() {
        let rt = to_rich_text("||Plan||Limit||Owner||\n|Free| |ana|");
        let Block::Table { rows, .. } = &rt.blocks[0] else { panic!("{:?}", rt.blocks) };
        assert_eq!(rows[1], [vec![text("Free")], vec![], vec![text("ana")]]);
    }

    #[test]
    fn what_follows_a_closing_macro_on_its_line_is_kept() {
        let rt =
            to_rich_text("{quote}Be careful{quote} see IC-1\n{code}\nx = 1{code} then restart");
        assert_eq!(
            rt.blocks,
            [
                Block::Quote(vec![para("Be careful")]),
                para("see IC-1"),
                Block::Code { language: None, text: "x = 1".into() },
                para("then restart"),
            ]
        );
    }
}
