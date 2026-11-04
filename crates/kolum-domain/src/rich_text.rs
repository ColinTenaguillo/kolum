//! Formatted text as a neutral tree.
//!
//! Every provider converts its own format (Jira's ADF, wikitext, GitHub's
//! Markdown) into this, and the UI renders only this. The tree is
//! deliberately smaller than any one format: what it cannot hold is
//! flattened on the way in (a table cell keeps its text, not its blocks),
//! which is the price of one renderer for every provider.

use crate::ids::UserId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RichText {
    pub blocks: Vec<Block>,
    /// Read from a richer format with something this one cannot hold (an
    /// image, a colour, an expand): written back, it would be lost.
    #[serde(default)]
    pub lossy: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Block {
    Paragraph(Vec<Inline>),
    Heading {
        level: u8,
        content: Vec<Inline>,
    },
    /// Each item is a list of blocks: an item can hold a nested list.
    List {
        ordered: bool,
        items: Vec<Vec<Block>>,
    },
    Code {
        language: Option<String>,
        text: String,
    },
    Quote(Vec<Block>),
    /// The first row is the header when `header` is set.
    Table {
        header: bool,
        rows: Vec<Vec<Vec<Inline>>>,
    },
    Panel {
        kind: PanelKind,
        blocks: Vec<Block>,
    },
    Rule,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PanelKind {
    Info,
    Note,
    Success,
    Warning,
    Error,
}

/// Inline formatting as flags on a run of text, the way ADF stores it,
/// rather than as nested nodes: `**_x_**` and `_**x**_` are the same text.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Marks {
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub strike: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Inline {
    Text { text: String, marks: Marks },
    Link { text: String, url: String },
    Mention { id: Option<UserId>, name: String },
    Emoji(String),
    Break,
}

impl Inline {
    pub fn text(s: impl Into<String>) -> Self {
        Inline::Text { text: s.into(), marks: Marks::default() }
    }
}

impl RichText {
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    /// Reads Markdown (CommonMark with GitHub's tables, strikethrough and
    /// `> [!NOTE]` alerts, which become panels). A link to `mention:<id>`
    /// is a mention, so a mention survives a trip through `$EDITOR`.
    pub fn from_markdown(source: &str) -> Self {
        markdown::parse(source)
    }

    /// Writes Markdown that [`RichText::from_markdown`] reads back into the
    /// same tree: the format of `$EDITOR`. Panels become GitHub alerts, so
    /// the five kinds survive; a table cell's line break becomes a space.
    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        for (i, block) in self.blocks.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            markdown_out::block(block, &mut out);
        }
        out
    }

    /// The text without its formatting: for search, and for a one-line
    /// summary where formatting has no room.
    pub fn plain_text(&self) -> String {
        let mut out = String::new();
        for block in &self.blocks {
            block_text(block, &mut out);
        }
        out.trim_end().to_string()
    }
}

fn block_text(block: &Block, out: &mut String) {
    match block {
        Block::Paragraph(inlines) | Block::Heading { content: inlines, .. } => {
            inline_text(inlines, out);
            out.push('\n');
        }
        Block::List { items, .. } => items.iter().flatten().for_each(|b| block_text(b, out)),
        Block::Code { text, .. } => {
            out.push_str(text);
            out.push('\n');
        }
        Block::Quote(blocks) | Block::Panel { blocks, .. } => {
            blocks.iter().for_each(|b| block_text(b, out))
        }
        Block::Table { rows, .. } => {
            for row in rows {
                for cell in row {
                    inline_text(cell, out);
                    out.push(' ');
                }
                out.push('\n');
            }
        }
        Block::Rule => {}
    }
}

pub fn inline_text(inlines: &[Inline], out: &mut String) {
    for inline in inlines {
        match inline {
            Inline::Text { text, .. } | Inline::Link { text, .. } | Inline::Emoji(text) => {
                out.push_str(text)
            }
            Inline::Mention { name, .. } => {
                out.push('@');
                out.push_str(name);
            }
            Inline::Break => out.push(' '),
        }
    }
}

mod markdown_out {
    use super::*;

    pub fn block(node: &Block, out: &mut String) {
        let line = |out: &mut String, text: &str| {
            out.push_str(text);
            out.push('\n');
        };
        match node {
            Block::Paragraph(inlines) => {
                for (i, l) in inlines_md(inlines).split('\n').enumerate() {
                    // A hard break: two spaces, then the next line.
                    if i > 0 {
                        out.pop();
                        out.push_str("  \n");
                    }
                    line(out, l);
                }
            }
            Block::Heading { level, content } => {
                line(out, &format!("{} {}", "#".repeat(*level as usize), inlines_md(content)))
            }
            // An item's blocks are written flush, then hung under the
            // bullet; a nested list nests by the same rule.
            Block::List { ordered, items } => {
                for (n, item) in items.iter().enumerate() {
                    let bullet = if *ordered { format!("{}. ", n + 1) } else { "- ".into() };
                    let pad = " ".repeat(bullet.len());
                    let mut text = String::new();
                    item.iter().for_each(|b| block(b, &mut text));
                    for (i, l) in text.lines().enumerate() {
                        let lead = if i == 0 { &bullet } else { &pad };
                        line(out, &if l.is_empty() { String::new() } else { format!("{lead}{l}") });
                    }
                }
            }
            Block::Code { language, text } => {
                line(out, &format!("```{}", language.as_deref().unwrap_or("")));
                text.lines().for_each(|l| line(out, l));
                line(out, "```");
            }
            Block::Quote(blocks) | Block::Panel { blocks, .. } => {
                let mut inner = String::new();
                if let Block::Panel { kind, .. } = node {
                    inner.push_str(&format!("[!{}]\n", alert(*kind)));
                }
                for (i, b) in blocks.iter().enumerate() {
                    if i > 0 {
                        inner.push('\n');
                    }
                    block(b, &mut inner);
                }
                for l in inner.lines() {
                    line(out, &if l.is_empty() { ">".to_string() } else { format!("> {l}") });
                }
            }
            Block::Table { rows, .. } => {
                for (r, row) in rows.iter().enumerate() {
                    let cells: Vec<String> = row
                        .iter()
                        .map(|c| inlines_md(c).replace('\n', " ").replace('|', "\\|"))
                        .collect();
                    line(out, &format!("| {} |", cells.join(" | ")));
                    if r == 0 {
                        line(out, &format!("|{}|", vec!["---"; row.len()].join("|")));
                    }
                }
            }
            Block::Rule => line(out, "---"),
        }
    }

    fn alert(kind: PanelKind) -> &'static str {
        match kind {
            PanelKind::Info => "IMPORTANT",
            PanelKind::Note => "NOTE",
            PanelKind::Success => "TIP",
            PanelKind::Warning => "WARNING",
            PanelKind::Error => "CAUTION",
        }
    }

    /// Characters Markdown would read as formatting, escaped.
    fn escape(text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        for c in text.chars() {
            if matches!(c, '\\' | '*' | '_' | '`' | '[' | ']' | '<' | '~' | '#' | '|') {
                out.push('\\');
            }
            out.push(c);
        }
        out
    }

    fn inlines_md(inlines: &[Inline]) -> String {
        let mut out = String::new();
        for inline in inlines {
            match inline {
                Inline::Text { text, marks } if marks.code => out.push_str(&format!("`{text}`")),
                Inline::Text { text, marks } => {
                    let (lead, trail) = (
                        &text[..text.len() - text.trim_start().len()],
                        &text[text.trim_end().len()..],
                    );
                    let core = escape(text.trim());
                    if core.is_empty() {
                        out.push_str(text);
                        continue;
                    }
                    let mut wrapped = core;
                    if marks.italic {
                        wrapped = format!("*{wrapped}*");
                    }
                    if marks.bold {
                        wrapped = format!("**{wrapped}**");
                    }
                    if marks.strike {
                        wrapped = format!("~~{wrapped}~~");
                    }
                    out.push_str(&format!("{lead}{wrapped}{trail}"));
                }
                Inline::Link { text, url } => out.push_str(&format!("[{}]({url})", escape(text))),
                Inline::Mention { id, name } => out.push_str(&format!(
                    "[@{}](mention:{})",
                    escape(name),
                    id.as_ref().map_or("", |i| i.as_str())
                )),
                Inline::Emoji(e) => out.push_str(e),
                Inline::Break => out.push('\n'),
            }
        }
        out
    }
}

mod markdown {
    use super::*;
    use pulldown_cmark::{BlockQuoteKind, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

    /// A container the parser is inside of, outermost first.
    enum Frame {
        Quote(Option<BlockQuoteKind>),
        List { ordered: bool, items: Vec<Vec<Block>> },
        Item,
        Table { rows: Vec<Vec<Vec<Inline>>>, row: Vec<Vec<Inline>> },
    }

    #[derive(Default)]
    struct Builder {
        /// One list of blocks per open block container, the root first.
        blocks: Vec<Vec<Block>>,
        frames: Vec<Frame>,
        /// The run of inlines being collected: a paragraph, a heading, a
        /// table cell, or the bare text of a tight list item.
        inlines: Option<Vec<Inline>>,
        heading: Option<u8>,
        code: Option<(Option<String>, String)>,
        link: Option<(String, String)>,
        bold: u8,
        italic: u8,
        strike: u8,
    }

    pub fn parse(source: &str) -> RichText {
        let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_GFM;
        let mut b = Builder { blocks: vec![Vec::new()], ..Default::default() };
        for event in Parser::new_ext(source, options) {
            b.event(event);
        }
        b.flush();
        RichText { blocks: b.blocks.pop().unwrap_or_default(), lossy: false }
    }

    impl Builder {
        fn push_block(&mut self, block: Block) {
            if let Some(top) = self.blocks.last_mut() {
                top.push(block);
            }
        }

        /// A tight list item's text arrives with no paragraph around it;
        /// it becomes one when the next block starts or the item ends.
        fn flush(&mut self) {
            if let Some(inlines) = self.inlines.take()
                && !inlines.is_empty()
            {
                self.push_block(Block::Paragraph(inlines));
            }
        }

        fn marks(&self, code: bool) -> Marks {
            Marks { bold: self.bold > 0, italic: self.italic > 0, code, strike: self.strike > 0 }
        }

        fn push_inline(&mut self, inline: Inline) {
            self.inlines.get_or_insert_with(Vec::new).push(inline);
        }

        fn text(&mut self, text: &str, code: bool) {
            if let Some((_, buf)) = &mut self.code {
                buf.push_str(text);
            } else if let Some((_, buf)) = &mut self.link {
                buf.push_str(text);
            } else {
                let marks = self.marks(code);
                self.push_inline(Inline::Text { text: text.to_string(), marks });
            }
        }

        fn event(&mut self, event: Event) {
            match event {
                Event::Start(tag) => self.start(tag),
                Event::End(tag) => self.end(tag),
                Event::Text(t) => self.text(&t, false),
                Event::Code(t) => self.text(&t, true),
                Event::SoftBreak => self.text(" ", false),
                Event::HardBreak => self.push_inline(Inline::Break),
                Event::Rule => {
                    self.flush();
                    self.push_block(Block::Rule);
                }
                _ => {}
            }
        }

        fn start(&mut self, tag: Tag) {
            match tag {
                Tag::Paragraph => {
                    self.flush();
                    self.inlines = Some(Vec::new());
                }
                Tag::Heading { level, .. } => {
                    self.flush();
                    self.heading = Some(level as u8);
                    self.inlines = Some(Vec::new());
                }
                Tag::BlockQuote(kind) => self.open(Frame::Quote(kind)),
                Tag::List(start) => {
                    self.flush();
                    self.frames.push(Frame::List { ordered: start.is_some(), items: Vec::new() });
                }
                Tag::Item => self.open(Frame::Item),
                Tag::CodeBlock(kind) => {
                    self.flush();
                    let lang = match kind {
                        CodeBlockKind::Fenced(l) if !l.is_empty() => Some(l.to_string()),
                        _ => None,
                    };
                    self.code = Some((lang, String::new()));
                }
                Tag::Table(_) => {
                    self.flush();
                    self.frames.push(Frame::Table { rows: Vec::new(), row: Vec::new() });
                }
                Tag::TableCell => self.inlines = Some(Vec::new()),
                Tag::Emphasis => self.italic += 1,
                Tag::Strong => self.bold += 1,
                Tag::Strikethrough => self.strike += 1,
                Tag::Link { dest_url, .. } => {
                    self.link = Some((dest_url.to_string(), String::new()))
                }
                _ => {}
            }
        }

        fn open(&mut self, frame: Frame) {
            self.flush();
            self.frames.push(frame);
            self.blocks.push(Vec::new());
        }

        fn end(&mut self, tag: TagEnd) {
            match tag {
                TagEnd::Paragraph => self.flush(),
                TagEnd::Heading(_) => {
                    let content = self.inlines.take().unwrap_or_default();
                    if let Some(level) = self.heading.take() {
                        self.push_block(Block::Heading { level, content });
                    }
                }
                TagEnd::BlockQuote(_) | TagEnd::Item => self.close(),
                TagEnd::List(_) => {
                    if let Some(Frame::List { ordered, items }) = self.frames.pop() {
                        self.push_block(Block::List { ordered, items });
                    }
                }
                TagEnd::CodeBlock => {
                    if let Some((language, mut text)) = self.code.take() {
                        if text.ends_with('\n') {
                            text.pop();
                        }
                        self.push_block(Block::Code { language, text });
                    }
                }
                TagEnd::TableCell => {
                    let cell = self.inlines.take().unwrap_or_default();
                    if let Some(Frame::Table { row, .. }) = self.frames.last_mut() {
                        row.push(cell);
                    }
                }
                TagEnd::TableHead | TagEnd::TableRow => {
                    if let Some(Frame::Table { rows, row }) = self.frames.last_mut() {
                        rows.push(std::mem::take(row));
                    }
                }
                TagEnd::Table => {
                    if let Some(Frame::Table { rows, .. }) = self.frames.pop() {
                        self.push_block(Block::Table { header: true, rows });
                    }
                }
                TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
                TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
                TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
                TagEnd::Link => {
                    if let Some((url, text)) = self.link.take() {
                        self.push_inline(link(url, text));
                    }
                }
                _ => {}
            }
        }

        /// Ends a quote or an item: its blocks go to whatever holds it.
        fn close(&mut self) {
            self.flush();
            let blocks = self.blocks.pop().unwrap_or_default();
            match self.frames.pop() {
                Some(Frame::Item) => {
                    if let Some(Frame::List { items, .. }) = self.frames.last_mut() {
                        items.push(blocks);
                    }
                }
                Some(Frame::Quote(kind)) => self.push_block(match kind {
                    None => Block::Quote(blocks),
                    Some(kind) => Block::Panel { kind: panel_kind(kind), blocks },
                }),
                _ => {}
            }
        }
    }

    fn link(url: String, text: String) -> Inline {
        match url.strip_prefix("mention:") {
            Some(id) => Inline::Mention {
                id: (!id.is_empty()).then(|| UserId::new(id)),
                name: text.trim_start_matches('@').to_string(),
            },
            None => Inline::Link { text, url },
        }
    }

    fn panel_kind(kind: BlockQuoteKind) -> PanelKind {
        match kind {
            BlockQuoteKind::Note => PanelKind::Note,
            BlockQuoteKind::Tip => PanelKind::Success,
            BlockQuoteKind::Important => PanelKind::Info,
            BlockQuoteKind::Warning => PanelKind::Warning,
            BlockQuoteKind::Caution => PanelKind::Error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bold(s: &str) -> Inline {
        Inline::Text { text: s.into(), marks: Marks { bold: true, ..Marks::default() } }
    }

    #[test]
    fn markdown_reads_headings_paragraphs_and_marks() {
        let rt = RichText::from_markdown("# Title\n\nSome **bold** and `code`.");
        assert_eq!(
            rt.blocks,
            vec![
                Block::Heading { level: 1, content: vec![Inline::text("Title")] },
                Block::Paragraph(vec![
                    Inline::text("Some "),
                    bold("bold"),
                    Inline::text(" and "),
                    Inline::Text {
                        text: "code".into(),
                        marks: Marks { code: true, ..Marks::default() }
                    },
                    Inline::text("."),
                ]),
            ]
        );
    }

    #[test]
    fn markdown_marks_nest_and_close_and_soft_breaks_are_spaces() {
        let rt = RichText::from_markdown("*it* ~~gone~~ **_both_** plain\nnext");
        let marks = |bold, italic, strike| Marks { bold, italic, code: false, strike };
        let run = |t: &str, m| Inline::Text { text: t.into(), marks: m };
        assert_eq!(
            rt.blocks,
            vec![Block::Paragraph(vec![
                run("it", marks(false, true, false)),
                Inline::text(" "),
                run("gone", marks(false, false, true)),
                Inline::text(" "),
                run("both", marks(true, true, false)),
                Inline::text(" plain"),
                Inline::text(" "),
                Inline::text("next"),
            ])]
        );
    }

    #[test]
    fn markdown_tight_and_nested_lists_keep_their_shape() {
        let rt = RichText::from_markdown("- one\n  1. inner\n- two");
        let Block::List { ordered: false, items } = &rt.blocks[0] else {
            panic!("expected a bullet list, got {:?}", rt.blocks)
        };
        assert_eq!(items.len(), 2);
        assert_eq!(items[0][0], Block::Paragraph(vec![Inline::text("one")]));
        assert!(matches!(&items[0][1], Block::List { ordered: true, items } if items.len() == 1));
        assert_eq!(items[1], vec![Block::Paragraph(vec![Inline::text("two")])]);
    }

    #[test]
    fn markdown_code_tables_panels_and_mentions() {
        let src = "```rust\nfn x() {}\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n\
                   > [!WARNING]\n> careful\n\nping [@Ana](mention:u2) and [docs](https://x.y)\n\n---";
        let rt = RichText::from_markdown(src);
        assert_eq!(
            rt.blocks[0],
            Block::Code { language: Some("rust".into()), text: "fn x() {}".into() }
        );
        assert_eq!(
            rt.blocks[1],
            Block::Table {
                header: true,
                rows: vec![
                    vec![vec![Inline::text("a")], vec![Inline::text("b")]],
                    vec![vec![Inline::text("1")], vec![Inline::text("2")]],
                ]
            }
        );
        assert_eq!(
            rt.blocks[2],
            Block::Panel {
                kind: PanelKind::Warning,
                blocks: vec![Block::Paragraph(vec![Inline::text("careful")])]
            }
        );
        assert_eq!(
            rt.blocks[3],
            Block::Paragraph(vec![
                Inline::text("ping "),
                Inline::Mention { id: Some(UserId::new("u2")), name: "Ana".into() },
                Inline::text(" and "),
                Inline::Link { text: "docs".into(), url: "https://x.y".into() },
            ])
        );
        assert_eq!(rt.blocks[4], Block::Rule);
    }

    /// Text runs side by side merge in a round trip ("a" "b" is "ab"),
    /// which is the same text; compare after merging them.
    fn merged(rt: RichText) -> RichText {
        fn inl(v: &[Inline]) -> Vec<Inline> {
            let mut out: Vec<Inline> = vec![];
            for i in v {
                match (out.last_mut(), i) {
                    (
                        Some(Inline::Text { text: a, marks: ma }),
                        Inline::Text { text: b, marks: mb },
                    ) if ma == mb => a.push_str(b),
                    _ => out.push(i.clone()),
                }
            }
            out
        }
        fn blk(b: &Block) -> Block {
            match b {
                Block::Paragraph(v) => Block::Paragraph(inl(v)),
                Block::Heading { level, content } => {
                    Block::Heading { level: *level, content: inl(content) }
                }
                Block::List { ordered, items } => Block::List {
                    ordered: *ordered,
                    items: items.iter().map(|i| i.iter().map(blk).collect()).collect(),
                },
                Block::Quote(v) => Block::Quote(v.iter().map(blk).collect()),
                Block::Panel { kind, blocks } => {
                    Block::Panel { kind: *kind, blocks: blocks.iter().map(blk).collect() }
                }
                Block::Table { header, rows } => Block::Table {
                    header: *header,
                    rows: rows.iter().map(|r| r.iter().map(|c| inl(c)).collect()).collect(),
                },
                other => other.clone(),
            }
        }
        RichText { blocks: rt.blocks.iter().map(blk).collect(), lossy: rt.lossy }
    }

    #[test]
    fn markdown_round_trips_through_the_editor_format() {
        let src = "## Steps\n\nAfter **SSO** the page says *expired*, see `a.rs` and ~~old~~ [docs](https://d.x).  \nNext line with a_b and 2*3.\n\n                   1. Sign in\n2. Wait\n   - 25 s\n   - more\n\n```log\nERROR x\nWARN y\n```\n\n> quoted\n\n> [!WARNING]\n> careful\n>\n> twice\n\n                   | Plan | Limit |\n|---|---|\n| Free | 60 |\n\nping [@Ana Silva](mention:u2)\n\n---";
        let first = RichText::from_markdown(src);
        let written = first.to_markdown();
        assert_eq!(merged(RichText::from_markdown(&written)), merged(first.clone()), "{written}");
        assert_eq!(
            merged(RichText::from_markdown(&RichText::from_markdown(&written).to_markdown())),
            merged(first)
        );
    }

    #[test]
    fn markdown_writes_every_panel_kind_and_escapes_what_would_format() {
        let panels: String = ["NOTE", "TIP", "IMPORTANT", "WARNING", "CAUTION"]
            .iter()
            .map(|k| format!("> [!{k}]\n> {k}\n\n"))
            .collect();
        let src = format!(
            "{panels}```\nplain code\n```\n\n| a\\|b | `c` |\n|---|---|\n| *x* | y |\n\n1\\*2 and \\_u\\_ and \\# not a heading"
        );
        let rt = RichText::from_markdown(&src);
        let written = rt.to_markdown();
        assert_eq!(merged(RichText::from_markdown(&written)), merged(rt), "{written}");
        assert!(written.contains("```\nplain code\n```"), "no language");
        assert!(written.contains(r"1\*2 and \_u\_"), "{written}");
    }

    #[test]
    fn plain_text_drops_formatting() {
        let rt = RichText::from_markdown("Hello **you** [@Ana](mention:u2)\n\n- a\n- b");
        assert_eq!(rt.plain_text(), "Hello you @Ana\na\nb");
        let every_block = RichText::from_markdown(
            "## H\n\n```\ncode\n```\n\n> quoted\n\n> [!TIP]\n> tip\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n---\n\nx  \ny",
        );
        assert_eq!(every_block.plain_text(), "H\ncode\nquoted\ntip\na b \n1 2 \nx y");
        assert!(RichText::default().is_empty() && !every_block.is_empty());
    }
}
