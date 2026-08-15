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
    /// The cloud has moved on and this machine has not — another machine, or
    /// another person, published something newer. Safe to take.
    RemoteAhead,
    /// Both sides changed since they last agreed. Neither can be applied over
    /// the other without losing work, so nothing is applied until someone says
    /// which one wins.
    Conflict,
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

/// Has this machine's copy changed since the two sides last agreed?
///
/// Separate from [`derive`] because a refresh needs the answer before it has
/// decided anything: a post with unpushed edits must not have its record
/// discarded, since the cached body those edits live in survives the refresh.
pub fn local_changed(sync: &post_sync::Model) -> bool {
    sync.synced_hash.as_deref() != Some(sync.local_hash.as_str())
}

/// Has the cloud's copy changed since the two sides last agreed?
///
/// Only answerable once a refresh has actually looked: without `remote_seen_at`
/// there is no observation to compare the baseline against, and "we have not
/// checked" is not the same as "nothing changed".
pub fn remote_changed(sync: &post_sync::Model) -> bool {
    sync.remote_seen_at.is_some() && sync.remote_seen_at != sync.remote_updated_at
}

/// Read a post's sync state off its staging and sync rows.
///
/// The two sides are compared independently and the four combinations are the
/// four states, which is the point: `Modified` and `RemoteAhead` are both safe
/// to act on automatically and in opposite directions, while their overlap is
/// the one case where acting at all destroys something.
///
/// ```text
///                 remote unchanged     remote changed
/// local unchanged      Clean            RemoteAhead
/// local changed       Modified           Conflict
/// ```
///
/// A failed push outranks all four: it is the state that needs a person, and it
/// already implies the local copy is not live.
///
/// A post with **no sync row** reads `Clean`. That is the honest answer rather
/// than a cautious one: the row is written by every path that changes content,
/// so its absence means nothing here has been touched since the post arrived —
/// which is the case for everything pulled from the cloud, and for the whole
/// library the first time this runs.
///
/// ## The one case that answer gets wrong
///
/// A post whose body was edited through MCP `update_draft` *before this table
/// existed* has a modified cached body and no row, so it reads `Clean` until
/// something saves it again.
///
/// That is knowingly left alone, because every way of catching it is worse.
/// Deciding at upgrade time whether a cached body differs from the live one
/// means downloading the whole blog from R2; doing it without downloading means
/// assuming, and the only safe assumption — every post with a cached body is
/// `Modified` — lights up an entire library with edits almost none of them
/// have. That is a louder lie than the quiet one it replaces. Reporting `Clean`
/// for what has not been checked at least says what the app said yesterday, and
/// one save of the affected post corrects it permanently.
pub fn derive(stage: Option<&post_stage::Model>, sync: Option<&post_sync::Model>) -> SyncState {
    if stage.is_some_and(|s| s.stage == post_stage::SYNC_FAILED) {
        return SyncState::SyncFailed;
    }
    let Some(sync) = sync else {
        return SyncState::Clean;
    };
    match (local_changed(sync), remote_changed(sync)) {
        (true, true) => SyncState::Conflict,
        (true, false) => SyncState::Modified,
        (false, true) => SyncState::RemoteAhead,
        (false, false) => SyncState::Clean,
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
            remote_updated_at: None,
            remote_seen_at: None,
        }
    }

    /// The same row, plus what the last refresh saw on the other side.
    fn seen(local: &str, synced: Option<&str>, baseline: i64, seen: i64) -> post_sync::Model {
        post_sync::Model {
            remote_updated_at: Some(baseline),
            remote_seen_at: Some(seen),
            ..sync(local, synced)
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

    /// The four combinations are the four states. `Modified` and `RemoteAhead`
    /// are each safe to act on automatically, in opposite directions; their
    /// overlap is the one case where acting at all destroys something.
    #[test]
    fn each_side_changing_gives_its_own_state() {
        // Nothing moved on either side.
        assert_eq!(derive(None, Some(&seen("v1", Some("v1"), 100, 100))), SyncState::Clean);
        // Only here.
        assert_eq!(derive(None, Some(&seen("v2", Some("v1"), 100, 100))), SyncState::Modified);
        // Only there.
        assert_eq!(derive(None, Some(&seen("v1", Some("v1"), 100, 200))), SyncState::RemoteAhead);
        // Both — the case that must not be resolved by guessing.
        assert_eq!(derive(None, Some(&seen("v2", Some("v1"), 100, 200))), SyncState::Conflict);
    }

    /// "We have not looked" is not "nothing changed". Until a refresh records
    /// what the cloud says, the remote side cannot be claimed to have moved.
    #[test]
    fn an_unobserved_remote_is_not_treated_as_changed() {
        let mut row = sync("v2", Some("v1"));
        row.remote_updated_at = Some(100);
        row.remote_seen_at = None;
        assert_eq!(derive(None, Some(&row)), SyncState::Modified);
        assert!(!remote_changed(&row));
    }

    /// A failed push outranks the comparison: it is the state that needs
    /// action, and it already implies the local copy is not live.
    #[test]
    fn a_failed_push_outranks_everything() {
        let failed = stage(post_stage::SYNC_FAILED);
        assert_eq!(derive(Some(&failed), Some(&sync("abc", Some("abc")))), SyncState::SyncFailed);
        assert_eq!(derive(Some(&failed), None), SyncState::SyncFailed);
        // Even over a conflict: a push that did not land needs a person first.
        assert_eq!(derive(Some(&failed), Some(&seen("v2", Some("v1"), 100, 200))), SyncState::SyncFailed);
    }
}
