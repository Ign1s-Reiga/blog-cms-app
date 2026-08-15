//! Whether a post's local content matches what readers are actually served.
//!
//! A post carries two independent facts, and the app used to conflate them:
//!
//! * **Publication state** — draft or published. An editorial decision.
//! * **Sync state** — whether what is on this machine is what is live. A fact
//!   about bytes.
//!
//! Nothing recorded the second one, so a published post edited locally — by the
//! editor, or by an MCP client through `update_draft`, which deliberately keeps
//! a published post published while saving new text locally — read as plainly
//! `Published`. The list said the post was live; it was, but not *this* version
//! of it.
//!
//! ## How "different" is decided
//!
//! By fingerprint, not by timestamp. `updated_at` moves whenever a post is
//! saved, so comparing times marks a post modified after a save that changed
//! nothing — opening a post, touching a character, and undoing it would leave a
//! badge claiming unpublished edits that do not exist. A hash over the content
//! answers the question actually being asked: *would a reader see anything
//! different?*

use sha2::{Digest, Sha256};

use crate::entities::post::Model as PostModel;
use crate::entities::post_stage;
use crate::entities::post_sync;

/// Where a post's local content stands relative to the cloud.
///
/// Serialized in snake_case for the frontend and for MCP clients, which both
/// render it directly.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SyncState {
    /// What is here is what is live — or the post has never been pushed and is
    /// not claiming otherwise.
    Clean,
    /// Edited since the last successful push. Readers are still being served the
    /// previous version.
    Modified,
    /// The last push was attempted and failed, so the local edits are not live
    /// and the cloud copy may be stale in a way nobody chose.
    SyncFailed,
}

/// Everything a reader would notice, as one fingerprint.
///
/// Deliberately excludes `updated_at`, `created_at` and ids: they move without
/// changing what is served, and a fingerprint that reacts to them would report
/// edits that are not there. `published` *is* included — unpublishing is a
/// change to what the reader gets.
pub fn content_hash(post: &PostModel, body: &str) -> String {
    let mut hasher = Sha256::new();
    // Length-prefixed so that moving text between adjacent fields cannot
    // produce the same digest — a title of "ab" with an empty excerpt would
    // otherwise hash like a title of "a" with excerpt "b".
    for field in [
        post.title.as_str(),
        post.excerpt.as_deref().unwrap_or(""),
        post.tags.as_deref().unwrap_or(""),
        if post.published { "1" } else { "0" },
        body,
    ] {
        hasher.update((field.len() as u64).to_le_bytes());
        hasher.update(field.as_bytes());
    }

    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Read a post's sync state off its staging and sync rows.
///
/// A failed push outranks everything: it is the one state that needs action,
/// and it already implies the local copy is not live. Otherwise the answer is
/// whether the fingerprints agree.
///
/// A post with **no sync row** reads `Clean`. That is the honest answer rather
/// than a cautious one: the row is written by every path that changes content,
/// so its absence means nothing here has been touched since the post arrived —
/// which is the case for everything pulled from the cloud, and for the whole
/// library the first time this runs.
pub fn derive(stage: Option<&post_stage::Model>, sync: Option<&post_sync::Model>) -> SyncState {
    if stage.is_some_and(|s| s.stage == post_stage::SYNC_FAILED) {
        return SyncState::SyncFailed;
    }
    match sync {
        Some(sync) if sync.synced_hash.as_deref() != Some(sync.local_hash.as_str()) => {
            SyncState::Modified
        }
        _ => SyncState::Clean,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn post(title: &str, published: bool) -> PostModel {
        PostModel {
            id: 1,
            slug: "a-post".into(),
            title: title.into(),
            excerpt: None,
            tags: None,
            published,
            published_at: None,
            series_id: None,
            series_order: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    fn sync(local: &str, synced: Option<&str>) -> post_sync::Model {
        post_sync::Model {
            post_id: 1,
            local_hash: local.into(),
            synced_hash: synced.map(str::to_string),
            synced_at: None,
        }
    }

    fn stage(stage: &str) -> post_stage::Model {
        post_stage::Model { post_id: 1, stage: stage.into(), staged_at: 0 }
    }

    /// The point of hashing rather than comparing timestamps: a save that
    /// changed nothing must not claim there are edits waiting.
    #[test]
    fn identical_content_hashes_identically() {
        let mut a = post("Title", true);
        let b = post("Title", true);
        assert_eq!(content_hash(&a, "body"), content_hash(&b, "body"));

        // Timestamps move on every save and must not register as a change.
        a.updated_at = 99_999;
        assert_eq!(content_hash(&a, "body"), content_hash(&b, "body"));
    }

    #[test]
    fn anything_a_reader_would_notice_changes_the_hash() {
        let base = content_hash(&post("Title", true), "body");
        assert_ne!(base, content_hash(&post("Other", true), "body"));
        assert_ne!(base, content_hash(&post("Title", true), "different body"));
        // Unpublishing changes what the reader gets, so it counts.
        assert_ne!(base, content_hash(&post("Title", false), "body"));

        let mut tagged = post("Title", true);
        tagged.tags = Some(r#"["rust"]"#.into());
        assert_ne!(base, content_hash(&tagged, "body"));
    }

    /// Field boundaries are hashed, so text sliding from one field to the next
    /// is not mistaken for no change at all.
    #[test]
    fn field_boundaries_are_part_of_the_fingerprint() {
        let mut a = post("ab", true);
        a.excerpt = Some(String::new());
        let mut b = post("a", true);
        b.excerpt = Some("b".into());
        assert_ne!(content_hash(&a, ""), content_hash(&b, ""));
    }

    #[test]
    fn matching_fingerprints_are_clean_and_differing_ones_are_modified() {
        assert_eq!(derive(None, Some(&sync("abc", Some("abc")))), SyncState::Clean);
        assert_eq!(derive(None, Some(&sync("abc", Some("old")))), SyncState::Modified);
    }

    /// A post that has never been pushed has local content the cloud has never
    /// seen, which is exactly what `Modified` means.
    #[test]
    fn a_never_pushed_post_counts_as_modified() {
        assert_eq!(derive(None, Some(&sync("abc", None))), SyncState::Modified);
    }

    /// Nothing recorded means nothing has been touched here — the state of
    /// every post pulled from the cloud, and of the whole library on upgrade.
    #[test]
    fn a_post_with_no_record_reads_clean() {
        assert_eq!(derive(None, None), SyncState::Clean);
        assert_eq!(derive(Some(&stage(post_stage::PUBLISHED)), None), SyncState::Clean);
    }

    /// A failed push outranks the comparison: it is the state that needs
    /// action, and it already implies the local copy is not live.
    #[test]
    fn a_failed_push_outranks_everything() {
        let failed = stage(post_stage::SYNC_FAILED);
        assert_eq!(derive(Some(&failed), Some(&sync("abc", Some("abc")))), SyncState::SyncFailed);
        assert_eq!(derive(Some(&failed), None), SyncState::SyncFailed);
    }
}
