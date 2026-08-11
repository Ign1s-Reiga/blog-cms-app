//! The R2 key convention for everything belonging to a post.
//!
//! This is a contract with the reader (`Ign1s-Reiga/blog`), which derives the
//! same keys in its own `src/lib/content.ts`. Both sides keep the convention in
//! exactly one file so a change here has one obvious counterpart there:
//!
//! ```text
//! posts/<slug>.md                  the body
//! posts/<slug>/thumbnail.avif      the card / og:image thumbnail
//! posts/<slug>/<sha256>.avif       an image used in the body
//! ```
//!
//! Body images are content-addressed rather than named, which makes an upload
//! idempotent: re-publishing an unchanged post rewrites the same key with the
//! same bytes, and the same image dropped into two posts is stored once per
//! post under a name both can compute.
//!
//! Inside the Markdown the reference is the **bare stored name**, not the full
//! key. The blog resolves it against the post's own prefix, so nothing bakes
//! the bucket's public host into stored content.

use sha2::{Digest, Sha256};

/// Everything for a post lives under this prefix.
pub fn media_prefix(slug: &str) -> String {
    format!("posts/{slug}/")
}

/// The post body's key.
pub fn body_key(slug: &str) -> String {
    format!("posts/{slug}.md")
}

/// The post thumbnail's key. Fixed name so the blog can build the URL from the
/// slug alone, with no lookup and no column in D1.
pub fn thumbnail_key(slug: &str) -> String {
    format!("{}thumbnail.avif", media_prefix(slug))
}

/// The stored name for a body image: its content hash plus extension. This is
/// what the Markdown references.
pub fn stored_name(bytes: &[u8], ext: &str) -> String {
    format!("{}.{ext}", hex_digest(bytes))
}

/// The full key for a body image stored under `slug`.
pub fn body_image_key(slug: &str, stored_name: &str) -> String {
    format!("{}{stored_name}", media_prefix(slug))
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
    fn keys_match_the_documented_layout() {
        assert_eq!(body_key("my-post"), "posts/my-post.md");
        assert_eq!(thumbnail_key("my-post"), "posts/my-post/thumbnail.avif");
        assert_eq!(body_image_key("my-post", "abc.avif"), "posts/my-post/abc.avif");
    }

    #[test]
    fn stored_name_is_the_content_hash() {
        // Known SHA-256 of "abc", so a change in hashing is caught here rather
        // than by every previously published post silently moving.
        let name = stored_name(b"abc", "avif");
        assert_eq!(
            name,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad.avif"
        );
    }

    #[test]
    fn identical_bytes_give_identical_names() {
        assert_eq!(stored_name(b"same", "avif"), stored_name(b"same", "avif"));
        assert_ne!(stored_name(b"one", "avif"), stored_name(b"two", "avif"));
    }
}
