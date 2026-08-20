//! Reading the YAML front matter an imported Markdown file may carry.
//!
//! The blog takes a post's metadata from D1, so front matter is never
//! authoritative here — a block left in a body publishes as a horizontal rule
//! followed by a heading made of the raw `title:`/`tags:` lines, which is why
//! [`split`] takes it out of the document either way. What it is good for is
//! sparing the author from retyping metadata the file already states, as a
//! proposal they confirm in the app.
//!
//! **Nothing in here returns an error.** An import must land whatever the file
//! opens with, so a block this module cannot make sense of yields no metadata
//! and the import falls back to the file name, exactly as it did before any of
//! this existed. Keys that are present but not read are reported in
//! [`FrontMatter::ignored`] rather than dropped in silence, so the confirm step
//! can say what it passed over instead of leaving the author to notice.
//!
//! This is deliberately **not** a YAML parser. It reads the four keys the app
//! has a column for, in the shapes those keys are actually written in:
//!
//! ```yaml
//! title: My Post              # or "My Post" / 'My Post'
//! tags: [rust, tauri]         # or a `- item` block, or `rust, tauri`
//! excerpt: One line about it
//! date: 2026-08-20            # or a full RFC 3339 timestamp
//! ```
//!
//! Anything else — nested mappings, multi-line scalars, anchors — is skipped
//! and named. A real YAML dependency would parse more and *reject* more, and
//! rejecting is the one thing this must not do.

use chrono::{DateTime, NaiveDate, NaiveTime, Utc};

/// The metadata an imported file proposes for itself.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct FrontMatter {
    pub title: Option<String>,
    pub tags: Vec<String>,
    pub excerpt: Option<String>,
    /// A Unix timestamp in seconds, to match the `created_at` column.
    pub date: Option<i64>,
    /// Keys the block sets that nothing here reads, in the order they appear.
    ///
    /// A `published: true` lands in this list like any other: an import is a
    /// local draft, and no file is allowed to talk the app into putting itself
    /// in front of readers.
    pub ignored: Vec<String>,
}

/// Split a document into the front matter it opens with and the body that
/// follows. The body is returned for every input, front matter or not.
///
/// The block must open on the very first line and close on a line of its own.
/// Without a closing delimiter the document is returned untouched, so a file
/// that merely starts with a `---` rule is not truncated.
pub fn split(content: &str) -> (Option<FrontMatter>, &str) {
    let Some(after_open) = content.strip_prefix("---") else {
        return (None, content);
    };
    // The opening `---` has to be alone on its line, or it is a rule or a
    // setext heading underline rather than a delimiter.
    let Some(rest) = after_open
        .strip_prefix('\n')
        .or_else(|| after_open.strip_prefix("\r\n"))
    else {
        return (None, content);
    };

    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if matches!(line.trim_end_matches(['\r', '\n']), "---" | "...") {
            let block = &rest[..offset];
            let body = rest[offset + line.len()..].trim_start_matches(['\r', '\n']);
            return (Some(parse(block)), body);
        }
        offset += line.len();
    }
    (None, content)
}

/// Read the keys the app has somewhere to put, and name the ones it does not.
fn parse(block: &str) -> FrontMatter {
    let mut fm = FrontMatter::default();
    let mut lines = block.lines().peekable();

    while let Some(line) = lines.next() {
        // Comments and blank lines carry nothing; a list item here belongs to
        // a key already consumed below.
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        // Indented: part of a structure whose key was already read (and
        // reported) — not a key of its own.
        if line.starts_with([' ', '\t']) {
            continue;
        }

        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = unquote(value.trim());

        match key.to_ascii_lowercase().as_str() {
            "title" if !value.is_empty() => fm.title = Some(value.to_string()),
            "excerpt" if !value.is_empty() => fm.excerpt = Some(value.to_string()),
            "tags" => {
                let tags = if value.is_empty() {
                    // A block list: the `- item` lines that follow.
                    collect_block_list(&mut lines)
                } else {
                    parse_inline_tags(value)
                };
                if tags.is_empty() {
                    fm.ignored.push(key.to_string());
                } else {
                    fm.tags = tags;
                }
            }
            "date" => match parse_date(value) {
                Some(ts) => fm.date = Some(ts),
                // A date in a shape this does not read is worth naming: it is
                // the difference between "no date given" and "your date was
                // passed over".
                None => fm.ignored.push(key.to_string()),
            },
            _ => fm.ignored.push(key.to_string()),
        }
    }

    fm
}

/// Consume the `- item` lines directly following a key with no inline value.
fn collect_block_list<'a>(lines: &mut std::iter::Peekable<impl Iterator<Item = &'a str>>) -> Vec<String> {
    let mut out = Vec::new();
    while let Some(next) = lines.peek() {
        let trimmed = next.trim();
        let Some(item) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix('-')) else {
            break;
        };
        let item = unquote(item.trim());
        if !item.is_empty() {
            out.push(item.to_string());
        }
        lines.next();
    }
    out
}

/// `[a, b]`, or a bare `a, b`. Both end up as the comma-separated string the
/// editor's tag field uses, so the two spellings are the same to everything
/// downstream.
fn parse_inline_tags(value: &str) -> Vec<String> {
    let inner = value
        .strip_prefix('[')
        .and_then(|v| v.strip_suffix(']'))
        .unwrap_or(value);
    inner
        .split(',')
        .map(|t| unquote(t.trim()))
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

/// A plain date, or a timestamp with a time on it. A bare date is taken at
/// midnight UTC — the app stores seconds, and a date with no time in it does
/// not say which zone it meant.
fn parse_date(value: &str) -> Option<i64> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(value) {
        return Some(dt.timestamp());
    }
    if let Ok(date) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        return Some(date.and_time(NaiveTime::MIN).and_utc().timestamp());
    }
    // Some writers emit `2026-08-20 09:30:00` with a space and no zone.
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S") {
        return Some(DateTime::<Utc>::from_naive_utc_and_offset(dt, Utc).timestamp());
    }
    None
}

/// Strip one layer of matching quotes. Escapes inside are left as written —
/// this reads metadata, it does not interpret it.
fn unquote(value: &str) -> &str {
    let bytes = value.as_bytes();
    if bytes.len() >= 2 && (bytes[0] == b'"' || bytes[0] == b'\'') && bytes[bytes.len() - 1] == bytes[0] {
        &value[1..value.len() - 1]
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fm(doc: &str) -> FrontMatter {
        split(doc).0.expect("front matter")
    }

    #[test]
    fn splits_block_from_body() {
        let doc = "---\ntitle: My Post\n---\n\nReal body.\n";
        let (found, body) = split(doc);
        assert_eq!(found.unwrap().title.as_deref(), Some("My Post"));
        assert_eq!(body, "Real body.\n");
    }

    #[test]
    fn reads_the_four_keys() {
        let doc = "---\ntitle: My Post\nexcerpt: About it\ntags: [rust, tauri]\ndate: 2026-08-20\n---\nBody\n";
        let f = fm(doc);
        assert_eq!(f.title.as_deref(), Some("My Post"));
        assert_eq!(f.excerpt.as_deref(), Some("About it"));
        assert_eq!(f.tags, vec!["rust", "tauri"]);
        assert_eq!(f.date, Some(1_787_184_000));
        assert!(f.ignored.is_empty());
    }

    #[test]
    fn reads_tags_in_every_spelling() {
        assert_eq!(fm("---\ntags: [a, b]\n---\n").tags, vec!["a", "b"]);
        assert_eq!(fm("---\ntags: a, b\n---\n").tags, vec!["a", "b"]);
        assert_eq!(fm("---\ntags:\n  - a\n  - b\n---\n").tags, vec!["a", "b"]);
        assert_eq!(fm("---\ntags:\n- \"a\"\n- 'b'\n---\n").tags, vec!["a", "b"]);
    }

    /// A block list must not swallow the key that follows it.
    #[test]
    fn a_block_list_ends_where_the_next_key_starts() {
        let f = fm("---\ntags:\n  - a\ntitle: After\n---\n");
        assert_eq!(f.tags, vec!["a"]);
        assert_eq!(f.title.as_deref(), Some("After"));
    }

    #[test]
    fn strips_one_layer_of_quotes() {
        assert_eq!(fm("---\ntitle: \"Quoted\"\n---\n").title.as_deref(), Some("Quoted"));
        assert_eq!(fm("---\ntitle: 'Quoted'\n---\n").title.as_deref(), Some("Quoted"));
        // Not a matching pair — left exactly as written.
        assert_eq!(fm("---\ntitle: \"Half\n---\n").title.as_deref(), Some("\"Half"));
    }

    #[test]
    fn reads_dates_with_and_without_a_time() {
        assert_eq!(fm("---\ndate: 2026-08-20\n---\n").date, Some(1_787_184_000));
        assert_eq!(
            fm("---\ndate: 2026-08-20T00:00:00Z\n---\n").date,
            Some(1_787_184_000)
        );
        assert_eq!(
            fm("---\ndate: 2026-08-20 00:00:00\n---\n").date,
            Some(1_787_184_000)
        );
    }

    /// The one key that must never be honoured. An import is a local draft, and
    /// a file does not get to publish itself.
    #[test]
    fn published_is_reported_not_obeyed() {
        let f = fm("---\ntitle: X\npublished: true\n---\n");
        assert_eq!(f.title.as_deref(), Some("X"));
        assert!(f.ignored.contains(&"published".to_string()));
    }

    #[test]
    fn names_what_it_did_not_read() {
        let f = fm("---\nlayout: post\nauthor: someone\ndate: last tuesday\n---\n");
        assert_eq!(f.ignored, vec!["layout", "author", "date"]);
        assert_eq!(f.date, None);
        assert_eq!(f.title, None);
        assert!(f.tags.is_empty());
    }

    /// A nested mapping's children are not keys in their own right.
    #[test]
    fn skips_the_inside_of_a_nested_key() {
        let f = fm("---\ntitle: X\nauthor:\n  name: someone\n  url: https://example.com\n---\n");
        assert_eq!(f.title.as_deref(), Some("X"));
        assert_eq!(f.ignored, vec!["author"]);
    }

    #[test]
    fn ignores_comments_and_blank_lines() {
        let f = fm("---\n# a comment\n\ntitle: X\n---\n");
        assert_eq!(f.title.as_deref(), Some("X"));
        assert!(f.ignored.is_empty());
    }

    #[test]
    fn handles_crlf() {
        let (found, body) = split("---\r\ntitle: X\r\n---\r\nBody\r\n");
        assert_eq!(found.unwrap().title.as_deref(), Some("X"));
        assert_eq!(body, "Body\r\n");
    }

    #[test]
    fn a_dot_delimiter_closes_the_block_too() {
        let (found, body) = split("---\ntitle: X\n...\nBody\n");
        assert_eq!(found.unwrap().title.as_deref(), Some("X"));
        assert_eq!(body, "Body\n");
    }

    /// Everything here has to come back as "no front matter, body untouched" —
    /// the shapes that would otherwise eat the top of somebody's document.
    #[test]
    fn leaves_documents_without_a_block_alone() {
        for doc in [
            "# Heading\n\nBody\n",
            "Body only\n",
            "",
            "----\nnot a delimiter\n",
            // Opens like a block but never closes: a horizontal rule.
            "---\n\nJust a rule, then prose.\n",
        ] {
            let (found, body) = split(doc);
            assert_eq!(found, None, "{doc:?} should not read as front matter");
            assert_eq!(body, doc);
        }
    }

    #[test]
    fn an_empty_block_is_a_block_that_proposes_nothing() {
        let (found, body) = split("---\n---\nBody\n");
        let f = found.expect("an empty block is still a block");
        assert_eq!(f, FrontMatter::default());
        assert_eq!(body, "Body\n");
    }
}
