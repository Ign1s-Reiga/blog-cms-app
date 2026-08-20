//! The MCP tool surface over the blog's own stores.
//!
//! Every tool here is a thin adapter: it validates what an MCP client sent,
//! then calls the same `commands`/`db` code the UI calls, so an agent and a
//! human editing the same post go through one implementation.
//!
//! The wire types (`PostOut`, `SeriesOut`, …) are deliberately separate from the
//! Sea ORM models. The models are a storage shape — `tags` is a JSON *string*,
//! `published` an integer — whereas a tool result should be the shape an agent
//! expects to read, and should not silently change when a column does.
//!
//! ## Write policy
//!
//! Drafting is unrestricted; publishing is not. Nothing in this file uploads to
//! R2 or upserts D1. `request_publish` only records intent in [`super::publish`]
//! — a human approves it in the app, and the approval path in [`super`] runs the
//! actual publish. See [`BlogMcp::get_info`] for the wording agents are told.

use std::path::PathBuf;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::{ServerCapabilities, ServerInfo};
use rmcp::{ErrorData, ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use sea_orm::{DatabaseConnection, TransactionTrait};
use serde::{Deserialize, Serialize};
use tauri::Manager;

use crate::commands;
use crate::db;
use crate::entities::post::Model as PostModel;
use crate::entities::post_stage;
use crate::entities::series::Model as SeriesModel;
use crate::sync_state::{self, SyncState};

use super::publish;

// ─── Wire types ───────────────────────────────────────────────────────────────

/// A post as an MCP client sees it: tags decoded, stage included.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PostOut {
    pub id: i32,
    pub slug: String,
    pub title: String,
    pub excerpt: Option<String>,
    pub tags: Vec<String>,
    /// Whether the blog considers this post live.
    pub published: bool,
    /// Unix seconds, set the first time the post was published.
    pub published_at: Option<i64>,
    pub series_id: Option<i32>,
    pub series_order: Option<i32>,
    pub created_at: i64,
    pub updated_at: i64,
    /// Local editorial stage: `draft`, `published`, or `sync_failed`.
    pub stage: Option<String>,
    /// Whether the local copy matches what readers are served: `clean`,
    /// `modified`, `remote_ahead`, `conflict`, or `sync_failed`.
    ///
    /// An agent editing a published post through `update_draft` moves this to
    /// `modified` — its changes are saved here and are not live until a human
    /// approves a publish. Reported so the agent can say so rather than assume
    /// the edit reached the blog.
    ///
    /// `conflict` means the cloud changed too, and nothing will be applied
    /// either way until a person picks a side in the app. An agent that finds
    /// one should say so and stop, not keep editing on top of it.
    pub sync_state: SyncState,
    /// The Markdown body. Only filled in by `get_post`; listing posts leaves it
    /// out so a large blog does not return megabytes of prose per call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SeriesOut {
    pub id: i32,
    pub slug: String,
    pub title: String,
    pub description: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct MediaOut {
    /// R2 object key, e.g. `media/3f2b….avif`.
    pub key: String,
    pub name: String,
    pub size: u64,
}

// ─── Tool parameters ──────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetPostParams {
    /// Id of the post to fetch.
    pub id: i32,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateDraftParams {
    /// Title of the new post. The slug is derived from it.
    pub title: String,
    /// Tags to file the post under.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Markdown body. May be empty and filled in by a later `update_draft`.
    #[serde(default)]
    pub body: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UpdateDraftParams {
    /// Id of the post to edit.
    pub id: i32,
    /// New title. Omit to leave it unchanged. The slug never changes, so links
    /// already published stay valid.
    #[serde(default)]
    pub title: Option<String>,
    /// Replacement tag list. Omit to leave tags unchanged.
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    /// Replacement Markdown body. Omit to leave the body unchanged.
    #[serde(default)]
    pub body: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RequestPublishParams {
    /// Id of the post to publish.
    pub post_id: i32,
    /// Why this should go live. Shown to the person approving it, so be specific.
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PublishStatusParams {
    /// The `id` returned by `request_publish`.
    pub request_id: String,
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn internal(e: impl std::fmt::Display) -> ErrorData {
    ErrorData::internal_error(e.to_string(), None)
}

fn invalid(e: impl std::fmt::Display) -> ErrorData {
    ErrorData::invalid_params(e.to_string(), None)
}

/// Decode the `tags` column (a JSON array) into a list.
fn tags_from_json(stored: Option<&str>) -> Vec<String> {
    stored
        .and_then(|t| serde_json::from_str::<Vec<String>>(t).ok())
        .unwrap_or_default()
}

/// Encode a tag list the way `save_post` expects it — comma-separated.
fn tags_to_csv(tags: &[String]) -> String {
    tags.iter()
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(",")
}

fn to_out(
    post: PostModel,
    stage: Option<String>,
    sync_state: SyncState,
    body: Option<String>,
) -> PostOut {
    PostOut {
        id: post.id,
        slug: post.slug,
        title: post.title,
        excerpt: post.excerpt,
        tags: tags_from_json(post.tags.as_deref()),
        published: post.published,
        published_at: post.published_at,
        series_id: post.series_id,
        series_order: post.series_order,
        created_at: post.created_at,
        updated_at: post.updated_at,
        stage,
        sync_state,
        body,
    }
}

/// The MCP server, one per session. Cheap to clone — it holds only the app
/// handle, and reads the database connection out of managed state on demand.
#[derive(Clone)]
pub struct BlogMcp {
    app: tauri::AppHandle,
    tool_router: ToolRouter<Self>,
}

impl BlogMcp {
    pub fn new(app: tauri::AppHandle) -> Self {
        Self { app, tool_router: Self::tool_router() }
    }

    fn conn(&self) -> tauri::State<'_, DatabaseConnection> {
        self.app.state::<DatabaseConnection>()
    }

    fn posts_dir(&self) -> Result<PathBuf, ErrorData> {
        Ok(self
            .app
            .path()
            .app_data_dir()
            .map_err(|e| internal(format!("Cannot resolve app data dir: {e}")))?
            .join("posts"))
    }

    /// One post by id, refusing anything in the trash.
    ///
    /// A trashed post is deleted as far as the app is concerned, and an agent
    /// editing or publishing one would be reaching around a decision the person
    /// made. Saying so is better than a bare "no such post": the id is real, and
    /// the agent may have read it from a listing taken before the delete.
    async fn load_post(&self, id: i32) -> Result<PostModel, ErrorData> {
        let post = db::get::<PostModel>(self.conn().inner(), id)
            .await
            .map_err(internal)?
            .ok_or_else(|| invalid(format!("No post with id {id}")))?;
        if db::trash_get(self.conn().inner(), id)
            .await
            .map_err(internal)?
            .is_some()
        {
            return Err(invalid(format!("Post {id} is in the trash")));
        }
        Ok(post)
    }

    /// A post's editorial stage and how its content compares with the cloud —
    /// the same two facts the desktop list shows, read the same way.
    async fn state_of(&self, post_id: i32) -> (Option<String>, SyncState) {
        let conn = self.conn();
        let stage = db::stage_get(conn.inner(), post_id).await.ok().flatten();
        let sync = db::sync_get(conn.inner(), post_id).await.ok().flatten();
        let state = sync_state::derive(stage.as_ref(), sync.as_ref());
        (stage.map(|s| s.stage), state)
    }
}

// `vis = "pub"` exposes the generated `BlogMcp::tool_router()` so the tool
// surface can be asserted from `tests/mcp_tools.rs`. That test lives outside the
// library for a Windows linking reason — see `build.rs`.
#[tool_router(router = tool_router, vis = "pub")]
impl BlogMcp {
    #[tool(
        description = "List every post in the local library with its metadata and editorial stage. Bodies are omitted; use get_post for one post's Markdown."
    )]
    pub async fn list_posts(&self) -> Result<Json<Vec<PostOut>>, ErrorData> {
        // Trash excluded, so an agent's view of the library is the same one the
        // person has in front of them.
        let posts = db::list_active_posts(self.conn().inner())
            .await
            .map_err(internal)?;

        let mut out = Vec::with_capacity(posts.len());
        for post in posts {
            let (stage, sync) = self.state_of(post.id).await;
            out.push(to_out(post, stage, sync, None));
        }
        Ok(Json(out))
    }

    #[tool(description = "Fetch one post by id, including its full Markdown body.")]
    pub async fn get_post(
        &self,
        Parameters(params): Parameters<GetPostParams>,
    ) -> Result<Json<PostOut>, ErrorData> {
        let post = self.load_post(params.id).await?;
        let (stage, sync) = self.state_of(post.id).await;
        // Goes through the command so a post whose body only exists in R2 is
        // downloaded and cached, exactly as it would be for the editor.
        let body = commands::read_post_markdown(self.app.clone(), self.conn(), post.slug.clone())
            .await
            .map_err(internal)?;
        Ok(Json(to_out(post, stage, sync, Some(body))))
    }

    #[tool(
        description = "Create a new post as a local draft. Nothing is uploaded: the draft stays on this machine until a publish is requested and a human approves it."
    )]
    pub async fn create_draft(
        &self,
        Parameters(params): Parameters<CreateDraftParams>,
    ) -> Result<Json<PostOut>, ErrorData> {
        let title = params.title.trim().to_string();
        if title.is_empty() {
            return Err(invalid("title must not be empty"));
        }

        // `published: false` is what keeps this local — see `save_post`.
        let saved = commands::save_post(
            self.app.clone(),
            self.conn(),
            None,
            title,
            tags_to_csv(&params.tags),
            params.body,
            false,
            // An agent's draft is filed by a person afterwards, from the editor
            // or the Series screen. The tool surface offers no series to set.
            None,
        )
        .await
        .map_err(internal)?;

        let (stage, sync) = self.state_of(saved.id).await;
        Ok(Json(to_out(saved, stage, sync, None)))
    }

    #[tool(
        description = "Edit an existing post's title, tags, or Markdown body. Changes are saved locally only; a published post keeps its current live version until a publish is requested and approved."
    )]
    pub async fn update_draft(
        &self,
        Parameters(params): Parameters<UpdateDraftParams>,
    ) -> Result<Json<PostOut>, ErrorData> {
        let mut post = self.load_post(params.id).await?;
        // Kept whole before the parameters are applied over it, so the snapshot
        // below records the post as it stood rather than half of this edit.
        let original = post.clone();

        // Which columns this edit is actually asking to change. Captured before
        // the parameters are consumed, and applied to a row re-read inside the
        // transaction — committing the model loaded above would carry every
        // *other* column back to how it looked before the awaits in between.
        let sets_title = params.title.is_some();
        let sets_tags = params.tags.is_some();

        if let Some(title) = params.title {
            let title = title.trim().to_string();
            if title.is_empty() {
                return Err(invalid("title must not be empty"));
            }
            post.title = title;
        }
        if let Some(tags) = params.tags {
            post.tags = Some(
                serde_json::to_string(
                    &tags
                        .iter()
                        .map(|t| t.trim())
                        .filter(|t| !t.is_empty())
                        .collect::<Vec<_>>(),
                )
                .map_err(internal)?,
            );
        }
        post.updated_at = chrono::Utc::now().timestamp();

        // This deliberately does not reuse `save_post`: that command takes the
        // published flag as an argument and would either push straight to the
        // cloud (bypassing the approval gate) or, passed `false`, quietly clear
        // `published` on a live post. Writing the local half here keeps a
        // published post's flag intact while its edits wait for approval.
        // Read from the row this edit actually lands on, not from the copy
        // loaded before the awaits below — see the merge in the transaction.
        let slug = post.slug.clone();

        // The body this post will have once the edit lands: the replacement when
        // one was sent, otherwise whatever is already there.
        //
        // Resolved *before* the metadata is committed, because the second case
        // can reach the network — an uncached body is fetched from R2 — and
        // failing after the commit would leave the row edited, the fingerprint
        // unwritten, and the post reporting itself clean while carrying changes.
        // Nothing here has been written yet, so failing is simply a no-op.
        let body = match &params.body {
            Some(body) => body.clone(),
            None => commands::read_post_markdown(self.app.clone(), self.conn(), slug.clone())
                .await
                .map_err(internal)?,
        };

        // An agent's edit is exactly the kind this history is for: it arrives
        // without anyone watching, and the post it lands on may be one a person
        // was in the middle of. Taken before the row is touched and while the
        // old body is still on disk, and best effort — an agent's edit is not
        // refused because a record of the previous version could not be kept.
        //
        // `original` rather than `post`: the latter already carries this edit's
        // title and tags.
        crate::revisions::snapshot_or_log(
            &self.app,
            self.conn().inner(),
            &original,
            crate::entities::post_revision::MCP,
        )
        .await;

        // An agent replacing a body is the same operation the editor performs,
        // so it takes the same lock — and takes it before the metadata below,
        // because the three writes are one commit sequence: metadata, body,
        // fingerprint. Acquiring it later would let this transaction commit
        // while an editor save holds the lock, and the two sequences would
        // interleave into a post carrying the editor's metadata, this body, and
        // a fingerprint computed from neither — with the editor's save
        // reporting success over a body that is gone.
        //
        // Ordered lock-then-database everywhere it is taken, so the paths that
        // hold it cannot deadlock against each other. Nothing under it reaches
        // the network. See `commands::lock_body_commits`.
        // Taken whether or not a body was sent. The fingerprint written at the
        // end of this function covers the body either way, so the metadata-only
        // path needs the same protection as the one that replaces text — it was
        // the guard's absence there that let an editor save land between this
        // edit's read and its fingerprint.
        let body_guard = commands::lock_body_commits().await;

        // Re-checked inside the transaction that writes. `load_post` refused a
        // trashed post several awaits ago — a body fetched from R2, a snapshot
        // taken — and an agent's edit is exactly the kind that arrives while
        // somebody is doing something else in the app.
        let conn = self.conn();
        let txn = conn.begin().await.map_err(internal)?;
        if db::trash_get(&txn, post.id).await.map_err(internal)?.is_some() {
            return Err(invalid(format!("Post {} is in the trash", post.id)));
        }
        // The row as it stands *now*, not as it stood before the body was
        // resolved and the snapshot taken. Those awaits can take as long as an
        // R2 download, and `into_update` writes every non-key column, so
        // committing the earlier copy would put back whatever changed meanwhile
        // — including `published`, which the owner may have set by pressing
        // Publish while this edit was in flight. The post would go on being
        // served while the app called it a draft that had never been published.
        let current = db::get::<PostModel>(&txn, post.id)
            .await
            .map_err(internal)?
            .ok_or_else(|| invalid(format!("Post {} no longer exists", post.id)))?;
        // From the fresh row for the same reason: the stage written at the end
        // of this function turns on it.
        let was_published = current.published;

        let mut merged = current;
        if sets_title {
            merged.title = post.title.clone();
        }
        if sets_tags {
            merged.tags = post.tags.clone();
        }
        merged.updated_at = post.updated_at;

        let saved = db::update::<PostModel>(&txn, merged).await.map_err(internal)?;
        txn.commit().await.map_err(internal)?;

        if params.body.is_some() {
            let dir = self.posts_dir()?;
            tokio::fs::create_dir_all(&dir)
                .await
                .map_err(|e| internal(format!("Failed to create posts dir: {e}")))?;
            // Staged and renamed rather than written in place, so a concurrent
            // read sees one whole body or the other and never half of one.
            let staged = commands::StagedBody::write(&dir, &body)
                .await
                .map_err(|e| internal(format!("Failed to write local markdown: {e}")))?;
            // Cleared before the rename, and allowed to fail the edit — see
            // `post_body_stale`, and the same ordering in `commands::r2::save`.
            // The other way round, a database outage after the rename leaves the
            // agent's text on disk still described as clean and stale, and the
            // next read fetches the published copy over it.
            db::body_stale_clear(self.conn().inner(), &slug)
                .await
                .map_err(internal)?;
            staged
                .commit(&dir.join(format!("{slug}.md")))
                .await
                .map_err(|e| internal(format!("Failed to write local markdown: {e}")))?;
        }

        // This is the edit the issue is about: a published post stays published
        // while its text changes here and nowhere else. Recording the local
        // fingerprint is what lets the app say so, and it is deliberately the
        // same call the desktop editor makes — one state model, not two.
        // Which body the fingerprint is about. When this call sent one, it is the
        // text just written. When it did not, `body` was read before the lock
        // and an editor save may have replaced the file since — so the copy on
        // disk is re-read here, under the lock, and that is a local read with
        // nothing slow about it.
        //
        // Hashing the stale copy instead would describe a version that is no
        // longer anywhere, and if it happened to match `synced_hash` the post
        // would read `clean` with an unpublished edit sitting in it.
        let fingerprinted = match &params.body {
            Some(sent) => sent.clone(),
            None => {
                let path = self.posts_dir()?.join(format!("{slug}.md"));
                tokio::fs::read_to_string(&path).await.unwrap_or_else(|_| body.clone())
            }
        };

        db::sync_set_local(
            self.conn().inner(),
            saved.id,
            crate::sync_state::content_hash(&saved, &fingerprinted),
        )
        .await
        .map_err(internal)?;

        // The file and the fingerprint agree from here on.
        drop(body_guard);

        // An unpublished post stays a draft. A published one keeps whatever
        // stage it had: it is still live with its old body, and the stage moves
        // only when an approved publish succeeds or fails.
        if !was_published {
            db::stage_set(
                self.conn().inner(),
                post_stage::Model {
                    post_id: saved.id,
                    stage: post_stage::DRAFT.to_string(),
                    staged_at: saved.updated_at,
                },
            )
            .await
            .map_err(internal)?;
        }

        let (stage, sync) = self.state_of(saved.id).await;
        Ok(Json(to_out(saved, stage, sync, None)))
    }

    #[tool(description = "List the series posts can be grouped into.")]
    pub async fn list_series(&self) -> Result<Json<Vec<SeriesOut>>, ErrorData> {
        let series = db::list::<SeriesModel>(self.conn().inner())
            .await
            .map_err(internal)?;
        Ok(Json(
            series
                .into_iter()
                .map(|s| SeriesOut {
                    id: s.id,
                    slug: s.slug,
                    title: s.title,
                    description: s.description,
                    created_at: s.created_at,
                })
                .collect(),
        ))
    }

    #[tool(
        description = "List media objects in the R2 library. Requires Cloudflare credentials to be configured in the app."
    )]
    pub async fn list_media(&self) -> Result<Json<Vec<MediaOut>>, ErrorData> {
        let items = commands::list_media(self.app.clone())
            .await
            .map_err(internal)?;
        Ok(Json(
            items
                .into_iter()
                .map(|m| MediaOut { key: m.key, name: m.name, size: m.size })
                .collect(),
        ))
    }

    #[tool(
        description = "Ask for a post to be published to the live blog. This does NOT publish it: it queues a request that a human must approve in the app. Poll publish_status with the returned id to find out what they decided."
    )]
    pub async fn request_publish(
        &self,
        Parameters(params): Parameters<RequestPublishParams>,
    ) -> Result<Json<publish::PublishRequest>, ErrorData> {
        let post = self.load_post(params.post_id).await?;

        // One open request per post: an agent that polls impatiently should not
        // be able to bury the approval list under duplicates of the same ask.
        if let Some(existing) = publish::open_for_post(post.id) {
            return Ok(Json(existing));
        }

        let request = publish::enqueue(post.id, post.slug, post.title, params.reason);
        super::notify_publish_change(&self.app);
        Ok(Json(request))
    }

    #[tool(
        description = "Check a publish request: awaiting_approval, publishing, rejected, published, or failed."
    )]
    pub async fn publish_status(
        &self,
        Parameters(params): Parameters<PublishStatusParams>,
    ) -> Result<Json<publish::PublishRequest>, ErrorData> {
        publish::get(&params.request_id)
            .map(Json)
            .ok_or_else(|| invalid(format!("No publish request {}", params.request_id)))
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for BlogMcp {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::new(ServerCapabilities::builder().enable_tools().build());
        info.instructions = Some(
            "Manage a personal blog stored on Cloudflare R2 (Markdown bodies and media) \
             and D1 (post metadata).\n\n\
             Drafting is free: create_draft and update_draft write only to this machine, \
             so you can iterate on a post without anything becoming visible to readers.\n\n\
             Publishing is gated. You cannot publish. request_publish records an ask that \
             the blog's owner approves or rejects in the desktop app; only their approval \
             uploads the body to R2 and updates D1. After calling it, report that approval \
             is pending rather than claiming the post is live, and use publish_status to \
             check back."
                .to_string(),
        );
        info
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_round_trip_through_the_stored_json_shape() {
        assert_eq!(tags_from_json(Some(r#"["rust","tauri"]"#)), vec!["rust", "tauri"]);
        assert_eq!(tags_to_csv(&["rust".into(), "tauri".into()]), "rust,tauri");
    }

    /// A missing or malformed column must read as "no tags" rather than failing
    /// the whole listing.
    #[test]
    fn undecodable_tags_are_empty_rather_than_fatal() {
        assert!(tags_from_json(None).is_empty());
        assert!(tags_from_json(Some("not json")).is_empty());
        assert!(tags_from_json(Some("")).is_empty());
    }

    #[test]
    fn blank_tags_are_dropped_on_the_way_out() {
        assert_eq!(tags_to_csv(&["  ".into(), "rust".into(), "".into()]), "rust");
        assert_eq!(tags_to_csv(&[]), "");
    }
}
