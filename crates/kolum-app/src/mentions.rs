//! `@name` in a comment, made a mention of whoever it names. The names
//! are read off the text as written; who they are is the provider's to say.

use kolum_domain::rich_text::{Block, Inline};
use kolum_domain::{RichText, User};

/// Whom a name is, among the users the provider found for it.
#[derive(Debug, Clone, PartialEq)]
pub enum Whom {
    /// No one: the name goes as written.
    Nobody,
    /// The one whose name it is, whatever the case: taken without asking.
    Named(User),
    /// The user picks: no one is called that, or more than one is.
    Choose(Vec<User>),
}

pub fn whom(name: &str, users: Vec<User>) -> Whom {
    let mut exact = users.iter().filter(|u| u.display_name.to_lowercase() == name.to_lowercase());
    match (exact.next(), exact.next()) {
        _ if users.is_empty() => Whom::Nobody,
        (Some(user), None) => Whom::Named(user.clone()),
        _ => Whom::Choose(users),
    }
}

/// The names written `@name` or `@"first last"`, outside code, each once,
/// in the order they come.
pub fn mentioned(text: &RichText) -> Vec<String> {
    let mut names: Vec<String> = vec![];
    // The walk is the one `mention` needs; a copy keeps the text untouched.
    runs(&mut text.blocks.clone(), &mut |runs| {
        for run in runs.iter() {
            for (_, name) in tokens(plain(run)) {
                if !names.iter().any(|n| n == name) {
                    names.push(name.to_string());
                }
            }
        }
    });
    names
}

/// Every `@name` of `name` made a mention of `user`.
pub fn mention(text: &mut RichText, name: &str, user: &User) {
    let at = Inline::Mention { id: Some(user.id.clone()), name: user.display_name.clone() };
    runs(&mut text.blocks, &mut |runs| {
        *runs = std::mem::take(runs)
            .into_iter()
            .flat_map(|run| {
                let Inline::Text { text, marks } = &run else { return vec![run] };
                let hits: Vec<_> =
                    tokens(plain(&run)).into_iter().filter(|t| t.1 == name).collect();
                if hits.is_empty() {
                    return vec![run];
                }
                let piece = |s: &str| Inline::Text { text: s.to_string(), marks: *marks };
                let mut out = vec![];
                let mut from = 0;
                for (span, _) in hits {
                    out.extend((span.start > from).then(|| piece(&text[from..span.start])));
                    out.push(at.clone());
                    from = span.end;
                }
                out.extend((from < text.len()).then(|| piece(&text[from..])));
                out
            })
            .collect();
    });
}

/// A run's text where a name can be written: not in code, not a link.
fn plain(run: &Inline) -> &str {
    match run {
        Inline::Text { text, marks } if !marks.code => text,
        _ => "",
    }
}

/// Each `@name` or `@"first last"` in `s`: where it is written, and the
/// name. An `@` right after a letter is an address, not a mention; a dot
/// or dash ending the name ends the sentence.
fn tokens(s: &str) -> Vec<(std::ops::Range<usize>, &str)> {
    let mut out = vec![];
    let mut prev = None;
    for (at, c) in s.char_indices() {
        let in_word = prev.is_some_and(char::is_alphanumeric);
        prev = Some(c);
        if c != '@' || in_word {
            continue;
        }
        let rest = &s[at + 1..];
        let (name, len) = match rest.strip_prefix('"').and_then(|q| q.split_once('"')) {
            Some((quoted, _)) => (quoted.trim(), quoted.len() + 2),
            None => {
                let word = rest.find(|c: char| !c.is_alphanumeric() && !"._-".contains(c));
                let name = rest[..word.unwrap_or(rest.len())].trim_end_matches(['.', '-']);
                (name, name.len())
            }
        };
        if !name.is_empty() {
            out.push((at..at + 1 + len, name));
        }
    }
    out
}

/// Every run of inlines in the blocks, however deep.
fn runs(blocks: &mut [Block], f: &mut impl FnMut(&mut Vec<Inline>)) {
    for block in blocks {
        match block {
            Block::Paragraph(content) | Block::Heading { content, .. } => f(content),
            Block::List { items, .. } => items.iter_mut().for_each(|item| runs(item, f)),
            Block::Quote(blocks) | Block::Panel { blocks, .. } => runs(blocks, f),
            Block::Table { rows, .. } => rows.iter_mut().flatten().for_each(&mut *f),
            Block::Code { .. } | Block::Rule => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kolum_domain::UserId;

    fn ana() -> User {
        User { id: UserId::new("557058:ana"), display_name: "Ana Silva".into(), avatar_url: None }
    }

    fn named(id: &str, name: &str) -> User {
        User { id: UserId::new(id), display_name: name.into(), avatar_url: None }
    }

    #[test]
    fn only_the_one_named_exactly_is_taken_without_asking() {
        let silvestre = named("u-silvestre", "Ana Silvestre");
        let both = vec![ana(), silvestre.clone()];
        assert_eq!(whom("ana silva", both.clone()), Whom::Named(ana()));
        assert_eq!(whom("ana", vec![ana()]), Whom::Choose(vec![ana()]), "a likeness is asked");
        assert_eq!(whom("ana", both.clone()), Whom::Choose(both));
        let twins = vec![ana(), named("u-other", "Ana Silva")];
        assert_eq!(whom("Ana Silva", twins.clone()), Whom::Choose(twins));
        assert_eq!(whom("ana", vec![]), Whom::Nobody);
    }

    #[test]
    fn names_come_once_in_order_and_never_from_code_links_or_addresses() {
        let text = RichText::from_markdown(
            "Thanks @ana, ask @\"Bob Martin\" and @ana.\n\n\
             - mail bob@x.io, run `@cmd`\n- see [@doc](https://d.x) or [@Ana](mention:u2)\n\n\
             ```\n@code\n```\n\n| who |\n|---|\n| @camille_t |\n\n> @dan-\n\n@ alone",
        );
        assert_eq!(mentioned(&text), ["ana", "Bob Martin", "camille_t", "dan"]);
    }

    #[test]
    fn each_mention_of_the_name_becomes_the_user_and_the_rest_stays() {
        let mut text = RichText::from_markdown("**hi @ana**, @anabel and @\"ana\" too");
        mention(&mut text, "ana", &ana());
        let at = Inline::Mention { id: Some(ana().id), name: "Ana Silva".into() };
        let bold = kolum_domain::rich_text::Marks { bold: true, ..Default::default() };
        let Block::Paragraph(runs) = &text.blocks[0] else { panic!("{text:?}") };
        assert_eq!(
            runs,
            &[
                Inline::Text { text: "hi ".into(), marks: bold },
                at.clone(),
                Inline::text(", @anabel and "),
                at,
                Inline::text(" too"),
            ]
        );
    }

    #[test]
    fn a_name_nowhere_in_the_text_leaves_it_as_it_was() {
        let mut text = RichText::from_markdown("no one here");
        let was = text.clone();
        mention(&mut text, "ana", &ana());
        assert_eq!(text, was);
    }
}
