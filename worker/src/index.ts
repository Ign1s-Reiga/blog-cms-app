/**
 * Publishes posts that were scheduled from the desktop app.
 *
 * The app may well be closed when a post falls due, so the publication cannot
 * depend on it: a cron trigger runs this Worker, and this Worker does the one
 * thing that has to happen at the appointed minute.
 *
 * ## Why this is only a database update
 *
 * Scheduling from the desktop app already uploaded everything: the body and its
 * images went to R2, and the metadata to D1 with `published` still 0. The blog
 * serves published rows only, so all of that is invisible to readers until this
 * runs. What is left is one flag.
 *
 * That is deliberate. A Worker that assembled the post at publication time would
 * need the R2 credentials, the image-rewriting rules and the local asset cache —
 * and it would need them at the one moment nobody is watching. Everything that
 * can fail is done while somebody is looking at it.
 *
 * ## Claim, then act
 *
 * A run claims a schedule by moving it to `publishing` before touching the post.
 * A cancellation that lands first wins, two overlapping runs cannot publish the
 * same post twice, and — the case a plain "mark it published up front" gets
 * wrong — a run that dies mid-flight leaves a row that says what actually
 * happened rather than one claiming a publication that never occurred.
 *
 * A claim that has sat in `publishing` past {@link STALE_CLAIM_SECONDS} is taken
 * to be from such a run and is retried. Without that, one crash would strand a
 * post short of the blog forever, and the only evidence would be a state nobody
 * is watching for.
 */

/** The slice of the D1 binding this Worker uses. */
interface D1Result<T> {
  results: T[];
}

interface D1RunResult {
  meta: { changes: number };
}

interface D1PreparedStatement {
  bind(...values: unknown[]): D1PreparedStatement;
  all<T>(): Promise<D1Result<T>>;
  run(): Promise<D1RunResult>;
}

interface D1Database {
  prepare(sql: string): D1PreparedStatement;
}

export interface Env {
  /** The same database the blog and the desktop app use. */
  DB: D1Database;
}

/** A schedule row that has come due. */
interface DueSchedule {
  slug: string;
  publish_at: number;
}

export interface PublishReport {
  published: string[];
  failed: string[];
  /** Claimed by another run, or cancelled between the query and the claim. */
  skipped: string[];
}

/**
 * How long a claim may sit in `publishing` before another run takes it over.
 *
 * Comfortably longer than a run takes — the work is one statement — and than the
 * gap between ticks, so a slow run is never overtaken by the next one.
 */
const STALE_CLAIM_SECONDS = 900;

/**
 * How many schedules one run will take on.
 *
 * Each costs up to three D1 statements, and a Worker invocation has a ceiling on
 * those — so an unbounded backlog (a first deployment, or a cron that was off
 * for a while) could run a single tick out of budget partway through, stranding
 * whichever schedule it had already claimed until the staleness rule frees it.
 *
 * A bounded batch fails softer: what is left is still `pending`, and the next
 * tick is five minutes away. `ORDER BY publish_at` means the batch is always the
 * longest-overdue ones, so nothing is starved by a backlog that keeps growing.
 */
const BATCH_LIMIT = 25;

/**
 * The posts table's name comes from the blog's own Drizzle schema and contains a
 * hyphen, so every reference to it has to be quoted.
 */
const POSTS = '"blog-db"';

/**
 * Publish everything that is due, and report what happened.
 *
 * Exported so the fetch handler can run the same pass: a fresh deployment can
 * then be verified without waiting for the next tick.
 */
export async function publishDue(env: Env, now: number): Promise<PublishReport> {
  const staleBefore = now - STALE_CLAIM_SECONDS;

  // Due and unclaimed, plus anything left claimed by a run that did not come
  // back. Both are selected here so the retry costs no extra round trip.
  const due = await env.DB.prepare(
    `SELECT slug, publish_at FROM post_schedule
      WHERE publish_at <= ?1
        AND (state = 'pending' OR (state = 'publishing' AND updated_at <= ?2))
      ORDER BY publish_at
      LIMIT ?3`,
  )
    .bind(now, staleBefore, BATCH_LIMIT)
    .all<DueSchedule>();

  if (due.results.length === BATCH_LIMIT) {
    // Said out loud, because a full batch is indistinguishable from a finished
    // one in the counts below — and a backlog that never drains is worth seeing.
    console.warn(
      `Batch limit of ${BATCH_LIMIT} reached; any remaining schedules wait for the next tick.`,
    );
  }

  const report: PublishReport = { published: [], failed: [], skipped: [] };

  for (const row of due.results) {
    // The claim. Repeating the conditions from the query is what makes it one:
    // a cancellation that landed since the SELECT, or another run that got here
    // first, leaves nothing to update and this run moves on.
    const claim = await env.DB.prepare(
      `UPDATE post_schedule SET state = 'publishing', updated_at = ?1
        WHERE slug = ?2
          AND (state = 'pending' OR (state = 'publishing' AND updated_at <= ?3))`,
    )
      .bind(now, row.slug, staleBefore)
      .run();

    if (claim.meta.changes === 0) {
      report.skipped.push(row.slug);
      continue;
    }

    try {
      const update = await env.DB.prepare(
        `UPDATE ${POSTS}
            SET published = 1,
                published_at = COALESCE(published_at, ?1),
                updated_at = ?1
          WHERE slug = ?2`,
      )
        .bind(now, row.slug)
        .run();

      if (update.meta.changes === 0) {
        // The schedule outlived the post it was for — deleted from D1, or its
        // slug changed. Recorded as a failure rather than passed over: a
        // publication that was asked for and did not happen is exactly what
        // somebody needs to be told about.
        await settle(env, row.slug, now, 'failed', 'No post with this slug exists in D1');
        report.failed.push(row.slug);
        continue;
      }

      await settle(env, row.slug, now, 'published', null);
      report.published.push(row.slug);
    } catch (error) {
      await settle(env, row.slug, now, 'failed', describe(error));
      report.failed.push(row.slug);
    }
  }

  return report;
}

/**
 * Record what became of a claimed schedule.
 *
 * Best effort: if this write fails there is nothing further to try, and throwing
 * would lose the outcomes of every schedule after this one. The row is left in
 * `publishing`, which the staleness rule above picks up on a later run.
 */
async function settle(
  env: Env,
  slug: string,
  now: number,
  state: 'published' | 'failed',
  error: string | null,
): Promise<void> {
  try {
    await env.DB.prepare(
      "UPDATE post_schedule SET state = ?1, error = ?2, updated_at = ?3 WHERE slug = ?4 AND state = 'publishing'",
    )
      .bind(state, error === null ? null : error.slice(0, 500), now, slug)
      .run();
  } catch (thrown) {
    console.error(`Could not record the outcome for ${slug}:`, describe(thrown));
  }
}

function describe(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** Unix seconds, which is how every timestamp in this schema is stored. */
function seconds(ms: number): number {
  return Math.floor(ms / 1000);
}

export default {
  async scheduled(event: { scheduledTime: number }, env: Env): Promise<void> {
    const report = await publishDue(env, seconds(event.scheduledTime));
    // The cron's own log is the only place anybody sees this, so say enough to
    // tell "nothing was due" from "nothing worked".
    console.log(
      `scheduled run: ${report.published.length} published, ${report.failed.length} failed, ${report.skipped.length} skipped`,
    );
  },

  /**
   * Nothing. The cron is the only way in.
   *
   * An HTTP trigger was tempting — it makes a fresh deployment easy to check —
   * but a Worker deployed to `workers.dev` is reachable by anyone who guesses
   * the name, and the trigger would let them decide when a publication runs.
   * The window is small (a post is only published once it is due anyway) and
   * the convenience is real, but neither is worth an endpoint that writes to the
   * blog's database and answers to no one.
   *
   * To check a deployment, run the cron handler yourself:
   * `wrangler dev --test-scheduled` then `curl 'http://localhost:8787/__scheduled'`.
   */
  async fetch(): Promise<Response> {
    return new Response('This Worker runs on a schedule and has no HTTP API.', {
      status: 404,
    });
  },
};
