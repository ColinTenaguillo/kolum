//! Atlassian Document Format (Jira Cloud's rich text) to the domain's
//! RichText. ADF is richer than RichText; what has no counterpart is
//! flattened to the nearest thing that keeps its words: an expand becomes a
//! quote under its title, a table cell keeps its text, a media node says
//! "[attachment]". Unknown nodes keep their children.

use kolum_domain::UserId;
use kolum_domain::rich_text::{Block, Inline, Marks, PanelKind, RichText};
use serde_json::{Value, json};

pub fn to_rich_text(doc: &Value) -> RichText {
    RichText { blocks: blocks(children(doc)) }
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
            level: attr(node, "level").and_then(Value::as_u64).unwrap_or(1).clamp(1, 6) as u8,
            content: inlines(children(node)),
        },
        "bulletList" | "orderedList" => Block::List {
            ordered: kind(node) == "orderedList",
            items: children(node).iter().map(|item| blocks(children(item))).collect(),
        },
        "taskList" | "decisionList" => {
            Block::List { ordered: false, items: children(node).iter().map(task).collect() }
        }
        "codeBlock" => Block::Code {
            language: attr_str(node, "language").filter(|l| !l.is_empty()).map(String::from),
            text: plain(children(node)),
        },
        "blockquote" => Block::Quote(blocks(children(node))),
        "panel" => Block::Panel {
            kind: panel_kind(attr_str(node, "panelType")),
            blocks: blocks(children(node)),
        },
        "rule" => Block::Rule,
        "table" => table(node),
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
    let text = || node.get("text").and_then(Value::as_str).unwrap_or("").to_string();
    let one = match kind(node) {
        "text" => return vec![marked(text(), node.get("marks"))],
        "hardBreak" => Inline::Break,
        "mention" => Inline::Mention {
            id: attr_str(node, "id").map(UserId::new),
            name: attr_str(node, "text").unwrap_or("someone").trim_start_matches('@').to_string(),
        },
        "emoji" => Inline::Emoji(
            attr_str(node, "text").or(attr_str(node, "shortName")).unwrap_or("").into(),
        ),
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
            "strong" => out.bold = true,
            "em" => out.italic = true,
            "code" => out.code = true,
            "strike" => out.strike = true,
            "link" => href = attr_str(mark, "href").map(String::from),
            _ => {}
        }
    }
    match href {
        Some(url) => Inline::Link { text, url },
        None => Inline::Text { text, marks: out },
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
        Block::Heading { level, content } => {
            json!({ "type": "heading", "attrs": { "level": level }, "content": inlines_out(content) })
        }
        Block::List { ordered, items } => node(
            if *ordered { "orderedList" } else { "bulletList" },
            items
                .iter()
                .map(|item| node("listItem", item.iter().map(block_out).collect()))
                .collect(),
        ),
        Block::Code { language, text } => {
            let content = if text.is_empty() {
                vec![]
            } else {
                vec![json!({ "type": "text", "text": text })]
            };
            json!({ "type": "codeBlock", "attrs": { "language": language }, "content": content })
        }
        Block::Quote(blocks) => node("blockquote", blocks.iter().map(block_out).collect()),
        Block::Panel { kind, blocks } => {
            let name = match kind {
                PanelKind::Info => "info",
                PanelKind::Note => "note",
                PanelKind::Success => "success",
                PanelKind::Warning => "warning",
                PanelKind::Error => "error",
            };
            json!({ "type": "panel", "attrs": { "panelType": name }, "content": blocks.iter().map(block_out).collect::<Vec<_>>() })
        }
        Block::Table { header, rows } => node(
            "table",
            rows.iter()
                .enumerate()
                .map(|(r, row)| {
                    let cell = if *header && r == 0 { "tableHeader" } else { "tableCell" };
                    let cells = row.iter().map(|c| node(cell, cell_paragraphs(c))).collect();
                    node("tableRow", cells)
                })
                .collect(),
        ),
        Block::Rule => json!({ "type": "rule" }),
    }
}

/// A cell's line breaks were paragraphs in ADF; they are again.
fn cell_paragraphs(inlines: &[Inline]) -> Vec<Value> {
    inlines.split(|i| *i == Inline::Break).map(|p| node("paragraph", inlines_out(p))).collect()
}

fn inlines_out(inlines: &[Inline]) -> Vec<Value> {
    inlines
        .iter()
        .filter_map(|inline| {
            Some(match inline {
                // ADF refuses an empty text node.
                Inline::Text { text, .. } | Inline::Link { text, .. } if text.is_empty() => return None,
                Inline::Text { text, marks } => {
                    let mut out = vec![];
                    for (on, kind) in [(marks.bold, "strong"), (marks.italic, "em"), (marks.code, "code"), (marks.strike, "strike")] {
                        if on {
                            out.push(json!({ "type": kind }));
                        }
                    }
                    if out.is_empty() {
                        json!({ "type": "text", "text": text })
                    } else {
                        json!({ "type": "text", "text": text, "marks": out })
                    }
                }
                Inline::Link { text, url } => {
                    json!({ "type": "text", "text": text, "marks": [{ "type": "link", "attrs": { "href": url } }] })
                }
                Inline::Mention { id, name } => {
                    json!({ "type": "mention", "attrs": { "id": id.as_ref().map(|i| i.as_str()), "text": format!("@{name}") } })
                }
                Inline::Emoji(e) => json!({ "type": "emoji", "attrs": { "shortName": e, "text": e } }),
                Inline::Break => json!({ "type": "hardBreak" }),
            })
        })
        .collect()
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
        let emoji = RichText { blocks: vec![Block::Paragraph(vec![Inline::Emoji("🎉".into())])] };
        assert_eq!(to_rich_text(&from_rich_text(&emoji)), emoji);
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
