-- The schedule of pending publications, read by the cron Worker and written by
-- the desktop app.
--
-- Deliberately its own table rather than a column on `blog-db`: that table's
-- shape belongs to the blog's schema, and the CMS builds its statements from it
-- column for column, so a new column there would have to be migrated everywhere
-- before the app could write anything at all.
--
-- Keyed by slug, which is the identity the local database, D1 and the blog all
-- agree on — row ids do not survive the crossing between them.
CREATE TABLE IF NOT EXISTS post_schedule (
  slug       TEXT PRIMARY KEY,
  -- Unix seconds, matching every other timestamp in this schema.
  publish_at INTEGER NOT NULL,
  -- pending | publishing | published | failed | cancelled
  state      TEXT    NOT NULL DEFAULT 'pending',
  -- Why the last attempt failed, when it did. Null otherwise.
  error      TEXT,
  updated_at INTEGER NOT NULL
);

-- Every cron tick asks the same question — what is due and unclaimed — and a
-- blog's worth of schedules is small, but the index costs nothing and keeps the
-- answer O(due) rather than O(all).
CREATE INDEX IF NOT EXISTS idx_post_schedule_due ON post_schedule (state, publish_at);
