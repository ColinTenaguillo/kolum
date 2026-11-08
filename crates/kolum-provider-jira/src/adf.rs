//! Atlassian Document Format (Jira Cloud's rich text) to the domain's
//! RichText. ADF is richer than RichText; what has no counterpart is
//! flattened to the nearest thing that keeps its words: an expand becomes a
//! quote under its title, a table cell keeps its text, a media node says
//! "[attachment]". Unknown nodes keep their children.

use kolum_domain::UserId;
use kolum_domain::rich_text::{Block, Inline, Marks, PanelKind, RichText};
use serde_json::{Value, json};

pub fn to_rich_text(doc: &Value) -> RichText {
    RichText { blocks: blocks(children(doc)), lossy: !faithful(doc) }
}

/// Whether every node and mark has a counterpart in RichText, so that the
/// text can be written back without losing anything.
fn faithful(node: &Value) -> bool {
    const NODES: [&str; 18] = [
        "doc",
        "paragraph",
        "heading",
        "bulletList",
        "orderedList",
        "listItem",
        "codeBlock",
        "blockquote",
        "panel",
        "rule",
        "table",
        "tableRow",
        "tableHeader",
        "tableCell",
        "text",
        "hardBreak",
        "mention",
        "emoji",
    ];
    const MARKS: [&str; 5] = ["strong", "em", "code", "strike", "link"];
    let marks = node.get("marks").and_then(Value::as_array).into_iter().flatten();
    NODES.contains(&kind(node))
        && marks.into_iter().all(|m| MARKS.contains(&kind(m)))
        && children(node).iter().all(faithful)
}

fn children(node: &Value) -> &[Value] {
    node.get("content").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default()
}

fn kind(node: &Value) -> &str {
    node.get("type").and_then(Value::as_str).unwrap_or("")
}

fn attr<'a>(node: &'a Value, name: &str) -> Option<&'a Value> {
    node.get("attrs").and_then(|a| a.get(name))
}

fn attr_str<'a>(node: &'a Value, name: &str) -> Option<&'a str> {
    attr(node, name).and_then(Value::as_str)
}

fn blocks(nodes: &[Value]) -> Vec<Block> {
    nodes.iter().flat_map(block).collect()
}

fn block(node: &Value) -> Vec<Block> {
    let one = match kind(node) {
        "paragraph" => Block::Paragraph(inlines(children(node))),
        "heading" => Block::Heading {
            level: attr(node, "level").and_then(Value::as_u64).map_or(1, |l| l.clamp(1, 6)) as u8,
            content: inlines(children(node)),
        },
        "bulletList" | "orderedList" => Block::List {
            ordered: kind(node) == "orderedList",
            items: children(node).iter().map(|item| blocks(children(item))).collect(),
        },
        "codeBlock" => Block::Code {
            language: attr_str(node, "language").filter(|l| !l.is_empty()).map(String::from),
            text: plain(children(node)),
        },
        "blockquote" | "panel" => quote(node),
        "rule" => Block::Rule,
        "table" => table(node),
        _ => return flattened(node),
    };
    vec![one]
}

/// A quote, or a panel by its type.
fn quote(node: &Value) -> Block {
    let blocks = blocks(children(node));
    match kind(node) {
        "panel" => Block::Panel { kind: panel_kind(attr_str(node, "panelType")), blocks },
        _ => Block::Quote(blocks),
    }
}

/// A node RichText has no counterpart for, as the nearest thing that keeps
/// its words; an unknown one keeps its children.
fn flattened(node: &Value) -> Vec<Block> {
    let one = match kind(node) {
        "taskList" | "decisionList" => {
            Block::List { ordered: false, items: children(node).iter().map(task).collect() }
        }
        "expand" | "nestedExpand" => {
            let title = attr_str(node, "title").unwrap_or("Details").to_string();
            let bold = Marks { bold: true, ..Marks::default() };
            let mut inner = vec![Block::Paragraph(vec![Inline::Text { text: title, marks: bold }])];
            inner.extend(blocks(children(node)));
            Block::Quote(inner)
        }
        "mediaSingle" | "mediaGroup" | "media" => {
            let marks = Marks { italic: true, ..Marks::default() };
            Block::Paragraph(vec![Inline::Text { text: "[attachment]".into(), marks }])
        }
        "blockCard" | "embedCard" => match attr_str(node, "url") {
            Some(url) => Block::Paragraph(vec![Inline::Link { text: url.into(), url: url.into() }]),
            None => return vec![],
        },
        _ => return blocks(children(node)),
    };
    vec![one]
}

/// A task or decision item: its text, with a box showing its state.
fn task(item: &Value) -> Vec<Block> {
    let done = matches!(attr_str(item, "state"), Some("DONE" | "DECIDED"));
    let mut line = vec![Inline::text(if done { "☑ " } else { "☐ " })];
    line.extend(inlines(children(item)));
    vec![Block::Paragraph(line)]
}

fn panel_kind(name: Option<&str>) -> PanelKind {
    match name {
        Some("note") => PanelKind::Note,
        Some("success") => PanelKind::Success,
        Some("warning") => PanelKind::Warning,
        Some("error") => PanelKind::Error,
        _ => PanelKind::Info,
    }
}

/// Rows of cells; a cell's paragraphs are joined by line breaks.
fn table(node: &Value) -> Block {
    let rows: Vec<&Value> = children(node).iter().filter(|r| kind(r) == "tableRow").collect();
    let header = rows.first().is_some_and(|r| children(r).iter().all(|c| kind(c) == "tableHeader"));
    let rows = rows
        .iter()
        .map(|row| {
            children(row)
                .iter()
                .map(|cell| {
                    let mut out = vec![];
                    for (i, paragraph) in children(cell).iter().enumerate() {
                        if i > 0 {
                            out.push(Inline::Break);
                        }
                        out.extend(inlines(children(paragraph)));
                    }
                    out
                })
                .collect()
        })
        .collect();
    Block::Table { header, rows }
}

fn inlines(nodes: &[Value]) -> Vec<Inline> {
    nodes.iter().flat_map(inline).collect()
}

fn inline(node: &Value) -> Vec<Inline> {
    let one = match kind(node) {
        "text" => {
            let text = node.get("text").and_then(Value::as_str).unwrap_or("").to_string();
            marked(text, node.get("marks"))
        }
        "hardBreak" => Inline::Break,
        "mention" => Inline::Mention {
            id: attr_str(node, "id").map(UserId::new),
            name: attr_str(node, "text").unwrap_or("someone").trim_start_matches('@').to_string(),
        },
        "emoji" => Inline::Emoji(
            attr_str(node, "text").or(attr_str(node, "shortName")).unwrap_or("").into(),
        ),
        _ => return flattened_inline(node),
    };
    vec![one]
}

/// An inline node RichText has no counterpart for, as text or a link; an
/// unknown one keeps its children.
fn flattened_inline(node: &Value) -> Vec<Inline> {
    let one = match kind(node) {
        "inlineCard" => match attr_str(node, "url") {
            Some(url) => Inline::Link { text: url.into(), url: url.into() },
            None => return vec![],
        },
        "status" => Inline::Text {
            text: format!("[{}]", attr_str(node, "text").unwrap_or("").to_uppercase()),
            marks: Marks { bold: true, ..Marks::default() },
        },
        "date" => Inline::text(date(attr_str(node, "timestamp"))),
        _ => return inlines(children(node)),
    };
    vec![one]
}

/// A text node with its marks. A link mark makes the whole run a link.
fn marked(text: String, marks: Option<&Value>) -> Inline {
    let mut out = Marks::default();
    let mut href = None;
    for mark in marks.and_then(Value::as_array).into_iter().flatten() {
        match kind(mark) {
            "link" => href = attr_str(mark, "href").map(String::from),
            other => set_mark(&mut out, other),
        }
    }
    match href {
        Some(url) => Inline::Link { text, url },
        None => Inline::Text { text, marks: out },
    }
}

/// A formatting mark; any other (a colour) is dropped.
fn set_mark(marks: &mut Marks, kind: &str) {
    match kind {
        "strong" => marks.bold = true,
        "em" => marks.italic = true,
        "code" => marks.code = true,
        "strike" => marks.strike = true,
        _ => {}
    }
}

fn plain(nodes: &[Value]) -> String {
    nodes.iter().filter_map(|n| n.get("text").and_then(Value::as_str)).collect()
}

/// ADF dates are milliseconds since the epoch, as a string.
fn date(ms: Option<&str>) -> String {
    ms.and_then(|ms| ms.parse::<i64>().ok())
        .and_then(|ms| time::OffsetDateTime::from_unix_timestamp(ms / 1000).ok())
        .map(|t| t.date().to_string())
        .unwrap_or_default()
}

/// The other way, for writes: RichText to an ADF document. What RichText
/// holds, ADF holds too, so this loses nothing.
pub fn from_rich_text(rt: &RichText) -> Value {
    json!({ "type": "doc", "version": 1, "content": rt.blocks.iter().map(block_out).collect::<Vec<_>>() })
}

fn node(kind: &str, content: Vec<Value>) -> Value {
    json!({ "type": kind, "content": content })
}

fn block_out(block: &Block) -> Value {
    match block {
        Block::Paragraph(inlines) => node("paragraph", inlines_out(inlines)),
        Block::Heading { level, content } => heading_out(*level, content),
        Block::List { ordered, items } => list_out(*ordered, items),
        Block::Code { language, text } => code_out(language.as_deref(), text),
        Block::Quote(blocks) => node("blockquote", blocks.iter().map(block_out).collect()),
        Block::Panel { kind, blocks } => panel_out(*kind, blocks),
        Block::Table { header, rows } => table_out(*header, rows),
        Block::Rule => json!({ "type": "rule" }),
    }
}

fn list_out(ordered: bool, items: &[Vec<Block>]) -> Value {
    let items = items.iter().map(|item| node("listItem", item.iter().map(block_out).collect()));
    node(if ordered { "orderedList" } else { "bulletList" }, items.collect())
}

fn heading_out(level: u8, content: &[Inline]) -> Value {
    json!({ "type": "heading", "attrs": { "level": level }, "content": inlines_out(content) })
}

fn panel_out(kind: PanelKind, blocks: &[Block]) -> Value {
    let content: Vec<Value> = blocks.iter().map(block_out).collect();
    json!({ "type": "panel", "attrs": { "panelType": panel_name(kind) }, "content": content })
}

/// ADF refuses an empty text node: an empty block has no content.
fn code_out(language: Option<&str>, text: &str) -> Value {
    let content =
        if text.is_empty() { vec![] } else { vec![json!({ "type": "text", "text": text })] };
    json!({ "type": "codeBlock", "attrs": { "language": language }, "content": content })
}

fn panel_name(kind: PanelKind) -> &'static str {
    match kind {
        PanelKind::Info => "info",
        PanelKind::Note => "note",
        PanelKind::Success => "success",
        PanelKind::Warning => "warning",
        PanelKind::Error => "error",
    }
}

fn table_out(header: bool, rows: &[Vec<Vec<Inline>>]) -> Value {
    let rows = rows.iter().enumerate().map(|(r, row)| {
        let cell = if header && r == 0 { "tableHeader" } else { "tableCell" };
        node("tableRow", row.iter().map(|c| node(cell, cell_paragraphs(c))).collect())
    });
    node("table", rows.collect())
}

/// A cell's line breaks were paragraphs in ADF; they are again.
fn cell_paragraphs(inlines: &[Inline]) -> Vec<Value> {
    inlines.split(|i| *i == Inline::Break).map(|p| node("paragraph", inlines_out(p))).collect()
}

fn inlines_out(inlines: &[Inline]) -> Vec<Value> {
    inlines.iter().filter_map(inline_out).collect()
}

fn inline_out(inline: &Inline) -> Option<Value> {
    Some(match inline {
        // ADF refuses an empty text node.
        Inline::Text { text, .. } | Inline::Link { text, .. } if text.is_empty() => return None,
        Inline::Text { text, marks } => text_out(text, marks),
        Inline::Link { text, url } => {
            json!({ "type": "text", "text": text, "marks": [{ "type": "link", "attrs": { "href": url } }] })
        }
        Inline::Mention { id, name } => {
            json!({ "type": "mention", "attrs": { "id": id.as_ref().map(|i| i.as_str()), "text": format!("@{name}") } })
        }
        Inline::Emoji(e) => json!({ "type": "emoji", "attrs": { "shortName": e, "text": e } }),
        Inline::Break => json!({ "type": "hardBreak" }),
    })
}

fn text_out(text: &str, marks: &Marks) -> Value {
    let flags = [
        (marks.bold, "strong"),
        (marks.italic, "em"),
        (marks.code, "code"),
        (marks.strike, "strike"),
    ];
    let out: Vec<Value> =
        flags.iter().filter(|(on, _)| *on).map(|(_, kind)| json!({ "type": kind })).collect();
    if out.is_empty() {
        json!({ "type": "text", "text": text })
    } else {
        json!({ "type": "text", "text": text, "marks": out })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rich_text_survives_a_trip_through_adf() {
        let rt = RichText::from_markdown(
            "## Steps\n\nAfter **SSO**, *see* `a.rs`, ~~old~~ [docs](https://d.x)  \nnext\n\n1. one\n   - inner\n2. two\n\n             ```rust\nfn x() {}\n```\n\n> quoted\n\n> [!CAUTION]\n> careful\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n             ping [@Ana](mention:u2)\n\n---",
        );
        assert_eq!(to_rich_text(&from_rich_text(&rt)), rt);
    }

    #[test]
    fn every_panel_kind_and_an_empty_code_block_survive() {
        let panels: String = ["NOTE", "TIP", "IMPORTANT", "WARNING", "CAUTION"]
            .iter()
            .map(|k| format!("> [!{k}]\n> {k}\n\n"))
            .collect();
        let rt = RichText::from_markdown(&format!("{panels}```\n```\n\n:tada: in text"));
        assert_eq!(to_rich_text(&from_rich_text(&rt)), rt);
        let emoji = RichText {
            blocks: vec![Block::Paragraph(vec![Inline::Emoji("🎉".into())])],
            lossy: false,
        };
        assert_eq!(to_rich_text(&from_rich_text(&emoji)), emoji);
    }

    #[test]
    fn what_adf_had_to_flatten_is_flagged() {
        let kept = RichText::from_markdown(
            "**hi** [@Ana](mention:u2)\n\n> [!NOTE]\n> n\n\n| a |\n|---|\n| 1 |",
        );
        assert!(!to_rich_text(&from_rich_text(&kept)).lossy, "what kolum writes reads back whole");
        let para = |inline: Value| json!({"type": "doc", "content": [{"type": "paragraph", "content": [inline]}]});
        let flattened = [
            json!({"type": "doc", "content": [{"type": "mediaSingle", "content": [{"type": "media"}]}]}),
            json!({"type": "doc", "content": [{"type": "expand", "attrs": {"title": "More"}}]}),
            para(json!({"type": "text", "text": "red", "marks": [{"type": "textColor"}]})),
            para(json!({"type": "status", "attrs": {"text": "done"}})),
            json!({"type": "doc", "content": [{"type": "paragraph", "marks": [{"type": "alignment"}]}]}),
        ];
        for doc in flattened {
            assert!(to_rich_text(&doc).lossy, "{doc}");
        }
    }

    #[test]
    fn a_heading_level_is_one_to_six() {
        let level = |attrs: Value| {
            let doc = json!({"type": "doc", "content": [{"type": "heading", "attrs": attrs}]});
            match &to_rich_text(&doc).blocks[0] {
                Block::Heading { level, .. } => *level,
                other => panic!("{other:?}"),
            }
        };
        assert_eq!(
            [level(json!({})), level(json!({"level": 0})), level(json!({"level": 9}))],
            [1, 1, 6]
        );
    }

    /// What has no counterpart keeps its words, in the marks that say what
    /// it was.
    #[test]
    fn flattened_nodes_keep_their_words_in_marks() {
        let bold = Marks { bold: true, ..Marks::default() };
        let italic = Marks { italic: true, ..Marks::default() };
        let run = |text: &str, marks| Inline::Text { text: text.into(), marks };
        let para =
            |text: &str| json!({"type": "paragraph", "content": [{"type": "text", "text": text}]});
        let doc = json!({"type": "doc", "content": [
            {"type": "taskList", "content": [{"type": "taskItem", "attrs": {"state": "DONE"}, "content": [{"type": "text", "text": "done"}]}]},
            {"type": "expand", "attrs": {"title": "More"}, "content": [para("inside")]},
            {"type": "mediaSingle"},
            {"type": "paragraph", "content": [{"type": "status", "attrs": {"text": "done"}}]},
            {"type": "table", "content": [{"type": "tableRow", "content": [
                {"type": "tableCell", "content": [para("a"), para("b")]}
            ]}]}
        ]});
        assert_eq!(
            to_rich_text(&doc).blocks,
            [
                Block::List {
                    ordered: false,
                    items: vec![vec![Block::Paragraph(vec![
                        Inline::text("☑ "),
                        Inline::text("done")
                    ])]]
                },
                Block::Quote(vec![
                    Block::Paragraph(vec![run("More", bold)]),
                    Block::Paragraph(vec![Inline::text("inside")]),
                ]),
                Block::Paragraph(vec![run("[attachment]", italic)]),
                Block::Paragraph(vec![run("[DONE]", bold)]),
                Block::Table {
                    header: false,
                    rows: vec![vec![vec![Inline::text("a"), Inline::Break, Inline::text("b")]]]
                },
            ]
        );
    }

    #[test]
    fn only_a_tables_first_row_is_its_header_and_a_break_splits_a_cell() {
        let cell = |t: &str| vec![Inline::text(t)];
        let rows =
            vec![vec![cell("h")], vec![vec![Inline::text("a"), Inline::Break, Inline::text("b")]]];
        let doc = from_rich_text(&RichText {
            blocks: vec![Block::Table { header: true, rows }],
            lossy: false,
        });
        let table = &doc["content"][0]["content"];
        assert_eq!(table[0]["content"][0]["type"], "tableHeader");
        assert_eq!(table[1]["content"][0]["type"], "tableCell");
        let paragraphs = &table[1]["content"][0]["content"];
        assert_eq!(paragraphs.as_array().map(Vec::len), Some(2), "{paragraphs}");
        assert_eq!(paragraphs[1]["content"][0]["text"], "b");
    }

    #[test]
    fn a_card_is_a_link_to_its_url_and_a_card_without_one_is_dropped() {
        let url = "https://x.y/1";
        let link = Inline::Link { text: url.into(), url: url.into() };
        let doc = |content: Value| json!({"type": "doc", "content": content});
        let block = json!([{"type": "blockCard", "attrs": {"url": url}}, {"type": "embedCard"}]);
        assert_eq!(to_rich_text(&doc(block)).blocks, [Block::Paragraph(vec![link.clone()])]);
        let inline = json!([{"type": "paragraph", "content": [
            {"type": "inlineCard", "attrs": {"url": url}}, {"type": "inlineCard"}
        ]}]);
        assert_eq!(to_rich_text(&doc(inline)).blocks, [Block::Paragraph(vec![link])]);
    }

    #[test]
    fn the_document_has_adf_shapes() {
        let doc = from_rich_text(&RichText::from_markdown("**hi** [@Ana](mention:u2)"));
        assert_eq!(doc["type"], "doc");
        assert_eq!(doc["version"], 1);
        let para = &doc["content"][0];
        assert_eq!(
            para["content"][0],
            json!({ "type": "text", "text": "hi", "marks": [{ "type": "strong" }] })
        );
        assert_eq!(
            para["content"][2],
            json!({ "type": "mention", "attrs": { "id": "u2", "text": "@Ana" } })
        );
        let empty = from_rich_text(&RichText::default());
        assert_eq!(empty["content"], json!([]));
    }
}
