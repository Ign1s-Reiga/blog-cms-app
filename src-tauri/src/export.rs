//! Writing a post out as a Markdown file with front matter.
//!
//! Writing is not the mirror of reading, and this module is deliberately not
//! the one that was removed in #113. Reading front matter meant guessing at
//! other people's conventions — `description` here, `publishDate` there — with
//! no end to the guessing. Writing has none of that problem: the schema below
//! is this app's own, every field comes from a column, and there is nothing to
//! interpret.
//!
//! What the file is for is being read by a person or by another tool. Nothing
//! in this app reads it back: import takes the file name and strips the block,
//! exactly as it did before. So an exported file is a copy, not a backup that
//! restores itself, and it is worth being clear about the difference.
//!
//! ```yaml
//! ---
//! title: "My Post"
//! slug: "my-post"
//! date: 2026-08-20T00:00:00Z
//! published: true
//! published_at: 2026-08-21T09:00:00Z
//! excerpt: "One line about it"
//! tags: ["rust", "tauri"]
//! series: "my-series"
//! series_order: 2
//! ---
//! ```
//!
//! A field the post does not have is left out rather than written empty, so the
//! block says only what is true.

use chrono::{DateTime, Utc};

use crate::entities::post::Model as PostModel;
use crate::entities::series::Model as SeriesModel;

/// Assemble the document: front matter, then the body as it stands.
///
/// `series` is the row the post's `series_id` names, when it names one. The
/// *slug* is written rather than the id, because an id is a local primary key
/// and means nothing in another database or another machine — the same reason
/// [`crate::db::SeriesMap`] exists.
pub fn document(post: &PostModel, series: Option<&SeriesModel>, body: &str) -> String {
    let mut out = String::from("---\n");

    push_str(&mut out, "title", &post.title);
    push_str(&mut out, "slug", &post.slug);
    push_line(&mut out, "date", &timestamp(post.created_at));
    push_line(&mut out, "published", if post.published { "true" } else { "false" });

    if let Some(at) = post.published_at {
        push_line(&mut out, "published_at", &timestamp(at));
    }
    if let Some(excerpt) = post.excerpt.as_deref().filter(|e| !e.is_empty()) {
        push_str(&mut out, "excerpt", excerpt);
    }

    let tags = tags_of(post);
    if !tags.is_empty() {
        let list: Vec<String> = tags.iter().map(|t| quote(t)).collect();
        push_line(&mut out, "tags", &format!("[{}]", list.join(", ")));
    }

    if let Some(series) = series {
        push_str(&mut out, "series", &series.slug);
        if let Some(order) = post.series_order {
            push_line(&mut out, "series_order", &order.to_string());
        }
    }

    out.push_str("---\n\n");
    out.push_str(body);
    // A file that does not end in a newline is a small rudeness to every tool
    // that reads it afterwards.
    if !body.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// The `tags` column is a JSON array; anything else in there is treated as no
/// tags rather than as an error, because an export is not the place to discover
/// that a row is malformed.
fn tags_of(post: &PostModel) -> Vec<String> {
    post.tags
        .as_deref()
        .and_then(|t| serde_json::from_str::<Vec<String>>(t).ok())
        .unwrap_or_default()
}

fn push_line(out: &mut String, key: &str, value: &str) {
    out.push_str(key);
    out.push_str(": ");
    out.push_str(value);
    out.push('\n');
}

fn push_str(out: &mut String, key: &str, value: &str) {
    push_line(out, key, &quote(value));
}

/// Always quoted, always escaped.
///
/// Deciding per value whether quoting is needed means knowing every character
/// YAML treats specially in every position — a colon, a leading `-`, `yes`,
/// `null`, a leading digit. Quoting unconditionally is uglier to read and right
/// every time.
///
/// **No control character is passed through.** A double-quoted YAML scalar may
/// not hold a raw C0 control, so one that reached a title — through MCP, or
/// pulled down from a row somebody else wrote — would produce a file no parser
/// accepts, which is exactly the audience this module exists for. The three
/// common ones get named escapes; everything else below `0x20`, and DEL, goes
/// out as a hex escape; and the three non-ASCII characters YAML reads as line
/// breaks get theirs, since raw they would end the scalar early.
fn quote(value: &str) -> String {
    use std::fmt::Write;

    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{85}' => out.push_str("\\N"),
            '\u{2028}' => out.push_str("\\L"),
            '\u{2029}' => out.push_str("\\P"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                let _ = write!(out, "\\x{:02x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Seconds to RFC 3339 in UTC. The column is a Unix timestamp, which names an
/// instant and not a zone, so writing one without saying `Z` would invite a
/// reader to guess.
fn timestamp(seconds: i64) -> String {
    DateTime::<Utc>::from_timestamp(seconds, 0)
        .unwrap_or_else(|| DateTime::<Utc>::from_timestamp(0, 0).expect("epoch is representable"))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn post() -> PostModel {
        PostModel {
            id: 1,
            slug: "my-post".to_string(),
            title: "My Post".to_string(),
            excerpt: None,
            tags: None,
            published: false,
            published_at: None,
            series_id: None,
            series_order: None,
            created_at: 1_787_184_000, // 2026-08-20T00:00:00Z
            updated_at: 1_787_184_000,
        }
    }

    fn series() -> SeriesModel {
        SeriesModel {
            id: 7,
            slug: "learning-rust".to_string(),
            title: "Learning Rust".to_string(),
            description: None,
            created_at: 0,
        }
    }

    #[test]
    fn a_draft_writes_only_what_it_has() {
        let doc = document(&post(), None, "Body text.\n");
        assert_eq!(
            doc,
            "---\n\
             title: \"My Post\"\n\
             slug: \"my-post\"\n\
             date: 2026-08-20T00:00:00Z\n\
             published: false\n\
             ---\n\
             \n\
             Body text.\n"
        );
    }

    #[test]
    fn everything_a_post_can_carry() {
        let mut p = post();
        p.excerpt = Some("One line about it".to_string());
        p.tags = Some(r#"["rust","tauri"]"#.to_string());
        p.published = true;
        p.published_at = Some(1_787_270_400);
        p.series_id = Some(7);
        p.series_order = Some(2);

        let doc = document(&p, Some(&series()), "Body.\n");
        assert!(doc.contains("published: true\n"));
        assert!(doc.contains("published_at: 2026-08-21T00:00:00Z\n"));
        assert!(doc.contains("excerpt: \"One line about it\"\n"));
        assert!(doc.contains("tags: [\"rust\", \"tauri\"]\n"));
        // The slug, not the id: 7 means nothing anywhere but this database.
        assert!(doc.contains("series: \"learning-rust\"\n"));
        assert!(!doc.contains("series: 7"));
        assert!(doc.contains("series_order: 2\n"));
    }

    /// The characters that would otherwise produce a block nothing can parse.
    #[test]
    fn values_that_would_break_the_block_are_escaped() {
        let mut p = post();
        p.title = r#"A: "quoted" \ thing"#.to_string();
        p.excerpt = Some("first\nsecond".to_string());

        let doc = document(&p, None, "Body\n");
        assert!(doc.contains(r#"title: "A: \"quoted\" \\ thing""#));
        assert!(doc.contains(r#"excerpt: "first\nsecond""#));
        // One line per key, whatever the values contained: an escaped newline
        // must not have become a real one and split the block.
        let block: Vec<&str> = doc.lines().skip(1).take_while(|l| *l != "---").collect();
        assert_eq!(block.len(), 5, "{block:?}");
        assert!(block.iter().all(|l| l.contains(": ")), "{block:?}");
    }

    /// Raw control characters make a double-quoted scalar invalid YAML, and they
    /// arrive through MCP or from rows written elsewhere — so the file the other
    /// tools are meant to read would be the one thing they cannot parse.
    #[test]
    fn control_characters_are_escaped_rather_than_written_raw() {
        let mut p = post();
        p.title = "nul\u{0}back\u{8}form\u{c}del\u{7f}".to_string();
        p.excerpt = Some("nel\u{85}sep\u{2028}para\u{2029}".to_string());

        let doc = document(&p, None, "Body\n");
        assert!(doc.contains(r#"title: "nul\x00back\x08form\x0cdel\x7f""#), "{doc}");
        assert!(doc.contains(r#"excerpt: "nel\Nsep\Lpara\P""#), "{doc}");

        // Nothing raw below 0x20 reached the block, whatever went in.
        let block: String = doc.lines().skip(1).take_while(|l| *l != "---").collect();
        assert!(
            !block.chars().any(|c| (c as u32) < 0x20 || c as u32 == 0x7f),
            "a raw control character reached the block"
        );
    }

    #[test]
    fn a_series_order_without_a_series_is_not_written() {
        let mut p = post();
        p.series_order = Some(3);
        let doc = document(&p, None, "Body\n");
        assert!(!doc.contains("series_order"));
    }

    #[test]
    fn a_body_without_a_trailing_newline_gets_one() {
        let doc = document(&post(), None, "No newline");
        assert!(doc.ends_with("No newline\n"));
    }

    #[test]
    fn a_malformed_tags_column_exports_as_no_tags() {
        let mut p = post();
        p.tags = Some("not json".to_string());
        let doc = document(&p, None, "Body\n");
        assert!(!doc.contains("tags:"));
    }
}
