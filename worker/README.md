# blog-cms-scheduler

The Cloudflare Worker that publishes scheduled posts.

The desktop app cannot do this itself: it may be closed when a post falls due,
and a timer inside an application that is not running is not a schedule. So the
app records _what_ should be published and _when_, and this Worker — on a cron
trigger — carries it out.

## What runs where

| Step                                       | Where           |
| ------------------------------------------ | --------------- |
| Body and images uploaded to R2             | Desktop app     |
| Metadata upserted into D1, `published` 0   | Desktop app     |
| `post_schedule` row written                | Desktop app     |
| `published` flipped to 1 at the right time | **This Worker** |
| Outcome recorded on the schedule row       | **This Worker** |

Everything that can fail — credentials, image rewriting, the upload itself —
happens while somebody is watching. What is left for the unattended moment is a
single `UPDATE`, which is why this Worker needs no R2 access and no secrets
beyond its database binding.

A post scheduled but not yet published is already fully present in R2 and D1; the
blog serves published rows only, so it is invisible to readers until the flip.

## Deploying

Both steps need the Cloudflare account that owns the blog's D1 database.

1. **Point the config at your database.** In `wrangler.jsonc`, replace
   `<your-d1-database>` and `<your-d1-database-id>` with the values from
   `pnpm exec wrangler d1 list`.

2. **Create the table.**

   ```sh
   pnpm exec wrangler d1 migrations apply <your-d1-database> --remote -c worker/wrangler.jsonc
   ```

   Until this has run, the desktop app's Refresh logs a warning about schedules
   and carries on — nothing else depends on the table existing.

3. **Deploy.**

   ```sh
   pnpm exec wrangler deploy -c worker/wrangler.jsonc
   ```

4. **Check it.** The Worker has no HTTP API — a `workers.dev` URL is reachable
   by anyone who guesses the name, and an endpoint that writes to the blog's
   database should not answer to strangers. Run the cron handler yourself
   instead:

   ```sh
   pnpm exec wrangler dev -c worker/wrangler.jsonc --test-scheduled
   curl 'http://localhost:8787/__scheduled'
   ```

   For the deployed copy, `pnpm exec wrangler tail -c worker/wrangler.jsonc`
   prints each run's summary as it happens.

## States

```text
pending ──claimed──> publishing ──> published
                          │
                          └────────> failed   (error is recorded on the row)
```

`cancelled` is written by the desktop app, and only ever from `pending`. A claim
that stays in `publishing` past fifteen minutes is taken to be from a run that
died and is retried — without that, one crash would strand a post short of the
blog for good.

The desktop app shows a `pending` row whose time has passed as **overdue**. That
state is not stored anywhere, because nothing is running to store it: it is what
"the cron has not run" looks like from the outside, and the usual causes are this
Worker not being deployed or its migration never having been applied.
