//! The R2 key layout for everything belonging to a post.
//!
//! The body's key is fixed:
//!
//! ```text
//! posts/<slug>.md
//! ```
//!
//! Media keys come from two configurable patterns, so the bucket can be laid
//! out to taste without a rebuild. Defaults:
//!
//! ```text
//! posts/{slug}/thumbnail.{ext}   the card / og:image thumbnail
//! posts/{slug}/{hash}.{ext}      an image used in the body
//! ```
//!
//! The two are separate because they are read differently, and that difference
//! decides how freely each may be changed:
//!
//! - **Body images** are safe to lay out however you like. The CMS writes their
//!   absolute URL into the published Markdown, so the reader never derives the
//!   key and cannot disagree about it.
//! - **The thumbnail** is derived by the reader from the slug alone — that is
//!   the point of its fixed name, and why no thumbnail column exists in D1. The
//!   pattern must therefore match `thumbnailKey` in the blog's
//!   `src/lib/content.ts`. A mismatch breaks thumbnails silently, because a
//!   missing object 404s rather than failing anything.
//!
//! Body images are content-addressed, which makes publishing idempotent:
//! re-publishing an unchanged post rewrites the same key with the same bytes.

use sha2::{Digest, Sha256};

use crate::error::{AppError, AppResult};

/// Default layout for a post's thumbnail. Must match the blog's `thumbnailKey`.
pub const DEFAULT_THUMBNAIL_PATTERN: &str = "posts/{slug}/thumbnail.{ext}";
/// Default layout for an image used in a post body.
pub const DEFAULT_MEDIA_PATTERN: &str = "posts/{slug}/{hash}.{ext}";

/// A strict, filesystem-safe slug: non-empty, only lowercase-friendly
/// alphanumerics plus `-`/`_` (no path separators, dots, or `..`).
pub fn is_safe_slug(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The post body's key. Not configurable: the reader fetches it through the
/// bucket binding by this exact name.
pub fn body_key(slug: &str) -> String {
    format!("posts/{slug}.md")
}

/// Expand `{slug}`, `{hash}` and `{ext}` in a key pattern.
///
/// Unknown placeholders are left as written rather than silently dropped, so a
/// typo shows up in the key instead of quietly collapsing two images onto one.
pub fn render(pattern: &str, slug: &str, hash: &str, ext: &str) -> String {
    pattern
        .replace("{slug}", slug)
        .replace("{hash}", hash)
        .replace("{ext}", ext)
        .trim_start_matches('/')
        .to_string()
}

/// The thumbnail's key under `pattern`.
pub fn thumbnail_key(pattern: &str, slug: &str, ext: &str) -> String {
    render(pattern, slug, "", ext)
}

/// The key for a body image under `pattern`, addressed by its content hash.
pub fn media_key(pattern: &str, slug: &str, bytes: &[u8], ext: &str) -> String {
    render(pattern, slug, &hex_digest(bytes), ext)
}

/// The public URL written into published Markdown for a body image.
///
/// Writing the URL out in full means the reader renders the Markdown as-is,
/// with no rewriting step and no need to know this layout — at the cost of a
/// domain change becoming a rewrite of every published post.
pub fn public_url(public_base: &str, key: &str) -> String {
    format!("{}/{}", public_base.trim_end_matches('/'), key)
}

/// Why a key pattern was rejected, phrased for display in Settings.
pub fn validate_pattern(pattern: &str, kind: PatternKind) -> AppResult<()> {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return Err(AppError::InvalidPattern("Pattern cannot be empty"));
    }
    if pattern.contains("..") {
        return Err(AppError::InvalidPattern("Pattern cannot contain `..`"));
    }
    if !pattern.contains("{slug}") {
        return Err(AppError::InvalidPattern(
            "Pattern must contain {slug}, or every post would share one key",
        ));
    }
    match kind {
        // Without the hash two different images in the same post collide, and
        // the second silently overwrites the first.
        PatternKind::Media if !pattern.contains("{hash}") => Err(AppError::InvalidPattern(
            "Media pattern must contain {hash}, or images in a post would overwrite each other",
        )),
        // The reader derives this from the slug alone; a hash makes it
        // underivable, so thumbnails would never be found.
        PatternKind::Thumbnail if pattern.contains("{hash}") => Err(AppError::InvalidPattern(
            "Thumbnail pattern cannot contain {hash} — the blog derives this key from the slug alone",
        )),
        _ => Ok(()),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PatternKind {
    Thumbnail,
    Media,
}

/// The content hash a body image is addressed by, as it appears in a key.
///
/// Public because it is the *identity* of a media object, not merely part of a
/// key: `media_usage` matches a library object against the copies of it that
/// travelled into posts, and the bytes are all they still have in common — the
/// staging step gives the copy a fresh name.
pub fn content_digest(bytes: &[u8]) -> String {
    hex_digest(bytes)
}

fn hex_digest(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_documented_layout() {
        assert_eq!(body_key("my-post"), "posts/my-post.md");
        assert_eq!(
            thumbnail_key(DEFAULT_THUMBNAIL_PATTERN, "my-post", "avif"),
            "posts/my-post/thumbnail.avif"
        );
        assert_eq!(
            media_key(DEFAULT_MEDIA_PATTERN, "my-post", b"abc", "avif"),
            "posts/my-post/ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad.avif"
        );
    }

    /// The default thumbnail pattern is a contract with the blog's
    /// `thumbnailKey`. If this changes, that file must change with it.
    #[test]
    fn default_thumbnail_key_matches_the_blog() {
        assert_eq!(
            thumbnail_key(DEFAULT_THUMBNAIL_PATTERN, "my-post", "avif"),
            "posts/my-post/thumbnail.avif"
        );
    }

    #[test]
    fn custom_patterns_expand() {
        assert_eq!(
            thumbnail_key("media/{slug}/cover.{ext}", "my-post", "avif"),
            "media/my-post/cover.avif"
        );
        // A leading slash would produce an empty first key segment in R2.
        assert_eq!(render("/a/{slug}.x", "p", "", ""), "a/p.x");
    }

    #[test]
    fn identical_bytes_give_identical_keys() {
        let k = |b: &[u8]| media_key(DEFAULT_MEDIA_PATTERN, "p", b, "avif");
        assert_eq!(k(b"same"), k(b"same"));
        assert_ne!(k(b"one"), k(b"two"));
    }

    #[test]
    fn public_url_joins_without_doubling_slashes() {
        let want = "https://cdn.example.com/posts/my-post/abc.avif";
        assert_eq!(public_url("https://cdn.example.com", "posts/my-post/abc.avif"), want);
        assert_eq!(public_url("https://cdn.example.com/", "posts/my-post/abc.avif"), want);
    }

    #[test]
    fn validation_rejects_patterns_that_would_lose_data() {
        use PatternKind::*;
        assert!(validate_pattern(DEFAULT_MEDIA_PATTERN, Media).is_ok());
        assert!(validate_pattern(DEFAULT_THUMBNAIL_PATTERN, Thumbnail).is_ok());

        // Two images in one post would collide on the same key.
        assert!(validate_pattern("posts/{slug}/img.{ext}", Media).is_err());
        // The blog could never derive a hashed thumbnail name.
        assert!(validate_pattern("posts/{slug}/{hash}.{ext}", Thumbnail).is_err());
        // Every post would share one key.
        assert!(validate_pattern("posts/thumb.{ext}", Thumbnail).is_err());
        assert!(validate_pattern("", Media).is_err());
        assert!(validate_pattern("../{slug}/{hash}.{ext}", Media).is_err());
    }
}
