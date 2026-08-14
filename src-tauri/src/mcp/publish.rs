//! The human-in-the-loop gate for publishes requested over MCP.
//!
//! An MCP client may ask for a post to go live, but nothing reaches R2 or D1
//! until someone approves the request in the app. A request is therefore only
//! ever a *record of intent*: [`enqueue`] stores it, the Settings screen lists
//! it, and `mcp_approve_publish` is what actually runs the publish.
//!
//! The queue is deliberately in-memory. An unapproved request should not
//! survive a restart — the person who would have approved it is no longer
//! looking at the session that produced it, and a stale request approved days
//! later would publish a body nobody re-read.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use schemars::JsonSchema;
use serde::Serialize;

use crate::error::{AppError, AppResult};

/// Where a publish request stands. Every state but
/// [`PublishState::AwaitingApproval`] is terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PublishState {
    /// Waiting for someone to approve or reject it in the app.
    AwaitingApproval,
    /// A human declined it. The post stays as it was.
    Rejected,
    /// Approved, and the publish succeeded.
    Published,
    /// Approved, but the publish itself failed — see `error`.
    Failed,
}

/// One request to publish a post, from the moment an MCP client asks until a
/// human resolves it.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct PublishRequest {
    /// Opaque id the MCP client polls with.
    pub id: String,
    pub post_id: i32,
    /// Captured when the request was made, so the approval UI can name the post
    /// without a database round-trip.
    pub slug: String,
    pub title: String,
    /// The justification the MCP client gave, shown to whoever decides.
    pub reason: Option<String>,
    pub requested_at: i64,
    pub state: PublishState,
    /// Why the publish failed, when `state` is `failed`.
    pub error: Option<String>,
}

/// Requests keyed by id.
///
/// A `std::sync::Mutex` is right here because every access is a brief map
/// operation; the guard is never held across an `.await` (the publish itself
/// runs after [`take_approved`] has returned and released it).
static QUEUE: OnceLock<Mutex<HashMap<String, PublishRequest>>> = OnceLock::new();

fn queue() -> &'static Mutex<HashMap<String, PublishRequest>> {
    QUEUE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A poisoned lock means another thread panicked mid-update. The map is a plain
/// collection with no cross-entry invariant, so recovering the guard is safe and
/// keeps one panic from disabling approvals for the rest of the session.
fn lock() -> std::sync::MutexGuard<'static, HashMap<String, PublishRequest>> {
    queue().lock().unwrap_or_else(|e| e.into_inner())
}

/// Record a new request and return it. The caller is expected to notify the
/// frontend so the approval shows up without a refresh.
pub fn enqueue(post_id: i32, slug: String, title: String, reason: Option<String>) -> PublishRequest {
    let request = PublishRequest {
        id: uuid::Uuid::new_v4().simple().to_string(),
        post_id,
        slug,
        title,
        reason: reason.filter(|r| !r.trim().is_empty()),
        requested_at: chrono::Utc::now().timestamp(),
        state: PublishState::AwaitingApproval,
        error: None,
    };
    lock().insert(request.id.clone(), request.clone());
    request
}

/// Look up a request by id.
pub fn get(id: &str) -> Option<PublishRequest> {
    lock().get(id).cloned()
}

/// Every request this session, newest first — what the Settings screen renders.
pub fn list() -> Vec<PublishRequest> {
    let mut all: Vec<PublishRequest> = lock().values().cloned().collect();
    all.sort_by(|a, b| b.requested_at.cmp(&a.requested_at).then(b.id.cmp(&a.id)));
    all
}

/// Whether a post already has a request waiting on a human.
///
/// Without this, an agent that polls impatiently could queue the same post a
/// dozen times and bury the approval list.
pub fn awaiting_for_post(post_id: i32) -> Option<PublishRequest> {
    lock()
        .values()
        .find(|r| r.post_id == post_id && r.state == PublishState::AwaitingApproval)
        .cloned()
}

/// Claim a pending request for execution, marking nothing yet.
///
/// Returns the request only if it was still awaiting approval, so two clicks on
/// Approve cannot publish twice. The caller then runs the publish and reports
/// back through [`settle`].
pub fn take_approved(id: &str) -> AppResult<PublishRequest> {
    let mut guard = lock();
    let request = guard
        .get_mut(id)
        .ok_or_else(|| AppError::NoPublishRequest(id.to_string()))?;
    match request.state {
        PublishState::AwaitingApproval => {
            // Left as-is until `settle` records the outcome; removing it from
            // the map here would lose the request if the publish then failed.
            Ok(request.clone())
        }
        state => Err(AppError::PublishRequestSettled { id: id.to_string(), state }),
    }
}

/// Reject a pending request. Returns the updated record.
pub fn reject(id: &str) -> AppResult<PublishRequest> {
    let mut guard = lock();
    let request = guard
        .get_mut(id)
        .ok_or_else(|| AppError::NoPublishRequest(id.to_string()))?;
    if request.state != PublishState::AwaitingApproval {
        return Err(AppError::PublishRequestSettled {
            id: id.to_string(),
            state: request.state,
        });
    }
    request.state = PublishState::Rejected;
    Ok(request.clone())
}

/// Record how an approved publish turned out.
pub fn settle(id: &str, outcome: Result<(), String>) -> Option<PublishRequest> {
    let mut guard = lock();
    let request = guard.get_mut(id)?;
    match outcome {
        Ok(()) => {
            request.state = PublishState::Published;
            request.error = None;
        }
        Err(e) => {
            request.state = PublishState::Failed;
            request.error = Some(e);
        }
    }
    Some(request.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ids must be unique, or one request's outcome would overwrite another's.
    #[test]
    fn enqueued_requests_get_distinct_ids() {
        let a = enqueue(1, "a".into(), "A".into(), None);
        let b = enqueue(1, "a".into(), "A".into(), None);
        assert_ne!(a.id, b.id);
        assert_eq!(a.state, PublishState::AwaitingApproval);
    }

    /// The guard that makes a double-click harmless: only the first claim wins.
    #[test]
    fn a_request_can_only_be_settled_once() {
        let req = enqueue(2, "b".into(), "B".into(), Some("  ".into()));
        // A blank reason is not a reason.
        assert_eq!(req.reason, None);

        assert!(take_approved(&req.id).is_ok());
        settle(&req.id, Ok(()));
        assert_eq!(get(&req.id).unwrap().state, PublishState::Published);

        // Already terminal, so neither path may re-enter it.
        assert!(take_approved(&req.id).is_err());
        assert!(reject(&req.id).is_err());
    }

    #[test]
    fn rejection_is_terminal_and_records_no_error() {
        let req = enqueue(3, "c".into(), "C".into(), None);
        let rejected = reject(&req.id).unwrap();
        assert_eq!(rejected.state, PublishState::Rejected);
        assert_eq!(rejected.error, None);
        assert!(reject(&req.id).is_err());
    }

    #[test]
    fn a_failed_publish_keeps_the_reason() {
        let req = enqueue(4, "d".into(), "D".into(), None);
        take_approved(&req.id).unwrap();
        settle(&req.id, Err("R2 unreachable".into()));
        let settled = get(&req.id).unwrap();
        assert_eq!(settled.state, PublishState::Failed);
        assert_eq!(settled.error.as_deref(), Some("R2 unreachable"));
    }

    /// Only unresolved requests block a new one for the same post.
    #[test]
    fn only_pending_requests_count_as_awaiting() {
        let req = enqueue(5, "e".into(), "E".into(), None);
        assert!(awaiting_for_post(5).is_some());
        reject(&req.id).unwrap();
        assert!(awaiting_for_post(5).is_none());
    }

    #[test]
    fn unknown_ids_are_reported_rather_than_panicking() {
        assert!(get("nope").is_none());
        assert!(take_approved("nope").is_err());
        assert!(reject("nope").is_err());
        assert!(settle("nope", Ok(())).is_none());
    }
}
