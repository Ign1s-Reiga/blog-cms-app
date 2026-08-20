# blog-cms-app

A desktop **CMS for a Cloudflare-hosted blog**, built with [Tauri](https://tauri.app) and
[Next.js](https://nextjs.org). Write in Markdown on your machine and publish to Cloudflare — **R2**
for bodies and images, **D1** for the metadata the blog queries.

The app is **local-first**: every post lives in a local SQLite database and is edited there, with no
network round-trip in the writing loop. Going to the cloud is an explicit act — push, publish, or
schedule — and the app tracks, per post, how the local copy compares with what readers are being
served.

> **Status — v1.6.4.** Posts, media, publishing, scheduling, revisions, trash, sync-conflict
> resolution, an MCP endpoint, and in-app updates all work end to end. The Analytics route is still a
> placeholder — the dashboard carries an R2/D1 usage card instead. See [Roadmap](#roadmap).
>
> **Windows only.** That is what the app targets, what the release workflow builds installers for,
> and what the credential storage is written against.

---

## Features

### Writing

- **Markdown editor** with a live split preview, formatting toolbar, list continuation, tab
  indentation, and drag-and-drop images.
- **Autosave** to this machine while you type. It saves the body only and carries no publish flag,
  so no timer can push anything live.
- **Revision history** — every edit snapshots the previous version first, and any snapshot can be
  restored.
- **Media picker** for inserting images from the R2 library, plus a per-post thumbnail.

### Library

- **Local-first posts list** with search and filters for All, Published, Drafts, Edited, Conflict,
  Scheduled, Failed, and Trash.
- **Trash** — deleting is a local soft delete. A published post that is trashed **stays live**;
  emptying the trash is the only path that is final.
- **Import** an existing `.md` file as a draft. YAML front matter is **stripped**, not read: the
  title comes from the file name and tags start empty, so metadata is re-entered in the app.
- **Series** for grouping related posts — modelled and synced, though there is no management screen
  yet (see [Roadmap](#roadmap)).

### Publishing

- **Explicit sync, both directions** — push local posts to D1, or pull the cloud's copy down.
  Conflicts are detected rather than silently resolved, and you choose which side wins.
- **Scheduled publishing** — pick a time and a [Cloudflare Worker](worker/README.md) flips the post
  live, whether or not the app is running.
- **AVIF conversion** — JPG and PNG uploads are re-encoded on the way into R2; formats that would
  lose animation or sharpness are stored byte-for-byte.
- **Media usage** — before deleting a library image, the app tells you which posts still depend on
  it, matched by content rather than by filename.

### Integrations

- **MCP server** — a local endpoint that lets an AI assistant read the library, draft, and edit,
  with publishing held behind an approval you give in the app.
- **R2/D1 usage analytics** on the dashboard, read from Cloudflare's GraphQL API.
- **In-app updates** from GitHub Releases, signed and verified before install.

## How it fits together

```text
        ┌─────────────────────────────┐
        │        blog-cms-app         │
        │  ┌───────────────────────┐  │
        │  │ local SQLite + posts/ │  │  ← the writing loop never leaves this box
        │  └───────────┬───────────┘  │
        └──────────────┼──────────────┘
                       │  push / pull / publish  (explicit)
          ┌────────────┴────────────┐
          ▼                         ▼
   ┌────────────┐          ┌──────────────┐
   │  R2 bucket │          │ D1 database  │ ◄── blog-cms-scheduler (cron Worker)
   │  bodies +  │          │   metadata   │     flips scheduled posts live
   │   images   │          └───────┬──────┘
   └──────┬─────┘                  │
          └───────────┬────────────┘
                      ▼
               the blog (reader)
```

The CMS does **not** own the D1 schema — the blog does. The `blog-db` and `series` tables come from
the blog's Drizzle schema, and this app mirrors them so both can use the same database. See
[Cloudflare configuration](#cloudflare-configuration).

## Tech stack

| Layer         | Technology                                                            |
| ------------- | --------------------------------------------------------------------- |
| Desktop shell | Tauri 2 (Rust 1.88+)                                                  |
| Frontend      | Next.js 16 (App Router, static export) · React 19                     |
| Styling       | Tailwind CSS v4 · shadcn/ui · lucide-react                            |
| Theming       | next-themes (dark / light)                                            |
| Markdown      | `@ign1s-reiga/marked-presets`                                         |
| Local store   | SQLite via Sea ORM · OS keychain (`keyring-core`) for secrets         |
| Cloud client  | reqwest (Cloudflare REST + GraphQL) · tokio · uuid · chrono           |
| Images        | `image` (AVIF encode) · sha2 (content addressing)                     |
| MCP           | `rmcp` (Streamable HTTP) · axum · schemars                            |
| Cloud         | Cloudflare R2 (bodies & media) · D1 (metadata) · Workers (scheduling) |
| Tooling       | pnpm · oxlint · oxfmt · wrangler                                      |

## Project structure

```text
blog-cms-app/
├── src/app/                     # Next.js App Router (frontend)
│   ├── components/              # UI components
│   │   ├── ui/                  # shadcn/ui primitives
│   │   ├── PostEditor.tsx       # editor: preview, autosave, media, schedule
│   │   ├── RevisionHistory.tsx  # snapshot list + rollback
│   │   ├── MediaPicker.tsx      # insert from the R2 library
│   │   ├── AuthGate.tsx         # gates the app on sign-in
│   │   ├── LoginScreen.tsx      # Cloudflare credentials form
│   │   ├── McpCard.tsx          # MCP settings + publish approvals
│   │   ├── AnalyticsCard.tsx    # R2/D1 usage
│   │   └── UpdateCard.tsx       # check / download / install
│   ├── lib/                     # cn(), sync helpers, updater client
│   ├── posts/                   # /posts, /posts/new, /posts/edit
│   ├── media/ analytics/ settings/
│   └── layout.tsx page.tsx globals.css
├── src-tauri/                   # Tauri backend (Rust)
│   ├── src/
│   │   ├── commands.rs          # shared helpers + command re-exports
│   │   ├── commands/
│   │   │   ├── local_db.rs      # local SQLite CRUD, import, trash
│   │   │   ├── d1.rs            # Cloudflare D1 writes, publish, schedule
│   │   │   └── r2.rs            # bodies, media library, staging
│   │   ├── entities/            # Sea ORM models (post, series, revision, …)
│   │   ├── auth.rs              # credentials: keychain + credentials.json
│   │   ├── cloudflare.rs        # R2 + D1 REST client
│   │   ├── sync_state.rs        # content hashing, conflict detection
│   │   ├── revisions.rs         # pre-edit snapshots
│   │   ├── imaging.rs           # JPG/PNG → AVIF
│   │   ├── media_keys.rs        # R2 key layout
│   │   ├── media_usage.rs       # which posts use a media object
│   │   ├── analytics.rs         # Cloudflare GraphQL usage data
│   │   ├── mcp/                 # MCP endpoint + gated publish queue
│   │   └── update.rs            # self-update
│   ├── tests/                   # MCP endpoint + tool-surface tests
│   ├── capabilities/            # Tauri permission definitions
│   └── tauri.conf.json
├── worker/                      # Cloudflare Worker: scheduled publishing
│   ├── src/index.ts
│   ├── migrations/
│   ├── wrangler.toml
│   └── README.md
└── .github/workflows/           # ci.yml, release.yml
```

## Prerequisites

- **Node.js 20.9+** (CI builds on 26) and **[pnpm](https://pnpm.io)** 11
- **Rust 1.88+** — install via [rustup](https://rustup.rs). Raised above Tauri's own floor by
  `rmcp`.
- **WebView2** and the **MSVC build tools** — see the
  [Tauri prerequisites guide](https://tauri.app/start/prerequisites/)
- **Access to GitHub Packages.** `@ign1s-reiga/marked-presets` is published there rather than to
  npm, so `pnpm install` needs a token with `read:packages`:

  ```ini
  # .npmrc (not committed)
  @ign1s-reiga:registry=https://npm.pkg.github.com/
  //npm.pkg.github.com/:_authToken=<your-github-token>
  ```

## Getting started

### 1. Install dependencies

```bash
pnpm install
```

### 2. Run the app

```bash
pnpm tauri dev
```

To work on the frontend alone in a browser, run `pnpm run dev` and open <http://localhost:3000>.
Every Tauri command — the whole data layer — is unavailable there, so the screens render empty.

### 3. Sign in

The app opens on a sign-in screen asking for your Cloudflare **account ID**, **API token**, **R2
bucket**, **D1 database ID**, and the bucket's **public URL**. The token goes to the OS keychain
(Windows Credential Manager); the rest is written to `credentials.json` in the app data directory.
If the credential store refuses the token, it goes into that file instead.

Nothing needs to be set in your shell — but if the credential store is empty at startup, the app
falls back to environment variables, which is convenient on a dev machine:

| Variable                   | Description                                          |
| -------------------------- | ---------------------------------------------------- |
| `CF_ACCOUNT_ID`            | Cloudflare account ID                                |
| `CF_API_TOKEN`             | API token (see permissions below)                    |
| `CF_R2_BUCKET`             | Target R2 bucket name                                |
| `CF_D1_DATABASE_ID`        | D1 database ID                                       |
| `CF_R2_PUBLIC_URL`         | Public origin the bucket is served from              |
| `CF_THUMBNAIL_KEY_PATTERN` | Optional; defaults to `posts/{slug}/thumbnail.{ext}` |
| `CF_MEDIA_KEY_PATTERN`     | Optional; defaults to `posts/{slug}/{hash}.{ext}`    |

## Cloudflare configuration

### API token permissions

| Permission                   | Needed for                                         |
| ---------------------------- | -------------------------------------------------- |
| **Workers R2 Storage: Edit** | Uploading bodies and media                         |
| **D1: Edit**                 | Reading and writing post metadata                  |
| **Account Analytics: Read**  | _Optional_ — the dashboard's R2/D1 usage card only |

A token without the analytics permission is perfectly valid for everything else, so the dashboard
says what is missing rather than showing an empty chart.

### D1 schema

The CMS shares a database with the blog and follows **the blog's** Drizzle schema — it does not
create or migrate these tables. Post rows live in `blog-db`, series in `series`. Timestamps are Unix
seconds, `tags` is a JSON-encoded string array, and `published` is stored as `0`/`1`.

| `blog-db`      | Type              |     | `series`      | Type           |
| -------------- | ----------------- | --- | ------------- | -------------- |
| `id`           | integer, PK       |     | `id`          | integer, PK    |
| `slug`         | text, unique      |     | `slug`        | text, unique   |
| `title`        | text              |     | `title`       | text           |
| `excerpt`      | text, nullable    |     | `description` | text, nullable |
| `tags`         | text, nullable    |     | `created_at`  | integer        |
| `published`    | integer 0/1       |     |               |                |
| `published_at` | integer, nullable |     |               |                |
| `series_id`    | integer, nullable |     |               |                |
| `series_order` | integer, nullable |     |               |                |
| `created_at`   | integer           |     |               |                |
| `updated_at`   | integer           |     |               |                |

One further table, `post_schedule`, **is** owned here — its migration ships with the Worker. See
[worker/README.md](worker/README.md).

### R2 key layout

| Key                            | What                                                     |
| ------------------------------ | -------------------------------------------------------- |
| `posts/<slug>.md`              | The post body. Fixed — the blog fetches it by this name. |
| `posts/<slug>/thumbnail.<ext>` | Thumbnail. Must match the blog's `thumbnailKey`.         |
| `posts/<slug>/<sha256>.<ext>`  | An image used in a body. Free to re-shape.               |
| `media/<uuid>.<ext>`           | The reusable media library.                              |

The **thumbnail** and **body image** patterns are configurable in `Settings → Media`. The body key
and the `media/` library prefix are not — the blog fetches the body by name, and the library prefix
is built into the code that lists and deletes from it.

Body images are **content-addressed**, which is what makes publishing idempotent: re-publishing an
unchanged post rewrites the same key with the same bytes. The thumbnail pattern is settable but not
free: the blog derives that key from the slug alone, so it must match `thumbnailKey` there. Change
one side without the other and thumbnails 404 silently, because a missing object is not an error
anybody sees.

## Local data

Everything the app owns lives in the Tauri app data directory:

| Path               | What                                                               |
| ------------------ | ------------------------------------------------------------------ |
| `blog-cms.db`      | SQLite: posts, series, staging, sync state, revisions, trash       |
| `posts/`           | Markdown bodies as files                                           |
| `assets/`          | Images staged into a post but not yet published                    |
| `media/`           | Local cache of the R2 media library                                |
| `credentials.json` | Non-secret Cloudflare settings (and the token, without a keychain) |
| `mcp.json`         | MCP endpoint settings                                              |

The schema is created from the Sea ORM entities at startup, so there is nothing to migrate by hand.

## How sync works

Publication and synchronisation are tracked as two separate facts, because a published post can be
carrying edits nobody has seen.

**Stage** is Draft or Published — what the blog is being told to serve.

**Sync state** is how the local copy compares with the cloud's, derived from a content hash over
everything a reader would notice (title, excerpt, tags, published flag, body):

| State           | Meaning                                                             |
| --------------- | ------------------------------------------------------------------- |
| **Clean**       | What is here is what is live.                                       |
| **Modified**    | Edited since the last push; readers still get the previous version. |
| **RemoteAhead** | The cloud moved on and this machine has not. Safe to take.          |
| **Conflict**    | Both sides changed. Nothing is applied until you say which wins.    |
| **SyncFailed**  | The last push failed, so the local edits are not live.              |

Four actions reach the cloud, and they do different amounts of work:

| Action                                | What it sends                                                 |
| ------------------------------------- | ------------------------------------------------------------- |
| **Publish** (editor) and **Schedule** | Staged images and the body to R2, then the metadata row to D1 |
| **Push to cloud** (header)            | Metadata only, for the whole library — upsert by slug         |
| **Publish** / **Unpublish** (list)    | Flips `published` in D1; no body is uploaded                  |
| **Refresh**                           | Mirrors D1 back down over the local cache                     |

Saving a draft stays entirely local. Publishing sends the images first — each rewritten to its
absolute URL under the public origin, which is what makes the Markdown the blog serves
self-contained — then the body, then the D1 row: R2 first, so a failure never leaves D1 pointing at
an object that is not there.

**Push to cloud is metadata only.** It is how a title, tag or series change reaches the blog without
re-uploading anything, but a post whose _body_ has been edited needs a publish before readers see the
new text.

## Scheduled publishing

The desktop app may be closed when a post falls due, so it records _what_ to publish and _when_, and
a Cloudflare Worker on a cron trigger carries it out. Everything that can fail — credentials, image
rewriting, the upload — happens while somebody is watching; what is left for the unattended moment is
a single `UPDATE`.

Scheduling therefore needs the Worker deployed and its migration applied, and the two are missed
differently:

- **No `post_schedule` table** (the migration was never applied) — scheduling **fails**. The app
  cannot record the schedule, so it undoes the local half and reports the error; the post stays a
  draft with no schedule against it. Its body is already in R2 by then, which is harmless: the blog
  serves published rows only. That undo is best effort, so if the local write fails too the schedule
  survives on this machine and does show as overdue — rare, and covered in
  [worker/README.md](worker/README.md).
- **Table present, Worker not running** — the schedule is recorded but nothing acts on it, and the
  post shows as **overdue** in the posts list once its time passes. That state is not stored
  anywhere; it is what "the cron has not run" looks like from the outside.

Setup is in [worker/README.md](worker/README.md).

## MCP server

`Settings → MCP server` hosts a Model Context Protocol endpoint so an assistant (Claude Desktop,
Claude Code, …) can work with the library:

| Tool                            | Does                                        |
| ------------------------------- | ------------------------------------------- |
| `list_posts` / `get_post`       | Read the library and one post's Markdown    |
| `create_draft` / `update_draft` | Write, locally only                         |
| `list_series` / `list_media`    | Read the groupings and the R2 media library |
| `request_publish`               | **Queue** a publish for a human to approve  |
| `publish_status`                | Check what was decided                      |

Nothing an agent does reaches readers on its own: `request_publish` only queues a request, which you
approve or reject in the same card.

The endpoint binds `127.0.0.1` (port **4127** by default, path `/mcp`) and requires
`Authorization: Bearer <token>`. Loopback keeps it off the network; the token keeps other local
software from using it just by knowing the port. The token is generated on first use and kept in the
OS keychain — or, where no keychain backend accepts it, written to `mcp.json` in plain text, the same
split the Cloudflare token falls back on. Point a client at:

```text
http://127.0.0.1:4127/mcp
```

## Releases & auto-update

The app updates itself from this repository's **GitHub Releases**.

**How it works.** Pushing a `v*` tag runs `.github/workflows/release.yml`, which builds the Windows
installers, signs the updater bundle, and attaches a `latest.json` manifest to a **draft** release.
Installed apps poll `releases/latest/download/latest.json`, compare its version against their own,
and verify the bundle's minisign signature before installing — so an update is only offered once you
**publish** the draft.

In the app, `Settings → Software update` shows the running version and drives check → download →
install → restart; the sidebar surfaces a notice when a check finds a newer version. Checks run once
per launch and are cached for the session.

**The changelog is written once.** The workflow generates it from the merged pull requests — the same
content the "Generate release notes" button produces — and it becomes both the draft's body and the
`notes` field in `latest.json`, which is what `Settings → Software update` displays under "Release
notes". The two therefore agree by construction, and the draft needs no notes written by hand.

Editing the draft's body afterwards does **not** reach `latest.json`: the manifest was uploaded
during the build, and nothing rewrites it. So an edit made before publishing leaves the updater
showing the generated version. If that matters for a particular release, edit the `latest.json` asset
to match, or re-run the workflow.

**One-time setup.** The workflow signs updates with a minisign key that must not live in the repo.
Generate one and add it to the repository secrets:

```bash
pnpm tauri signer generate -w ~/.tauri/blog-cms-app.key
```

| Secret                               | Value                            |
| ------------------------------------ | -------------------------------- |
| `TAURI_SIGNING_PRIVATE_KEY`          | Contents of the private key file |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | The passphrase you chose         |

Give the key a real passphrase — GitHub rejects empty secret values, so a passphrase-less key has
nothing valid to put in the second secret.

The matching **public** key lives in `src-tauri/tauri.conf.json` under `plugins.updater.pubkey`. The
two must stay paired: replacing the key means already-installed apps can no longer verify updates and
have to be reinstalled manually. Keep a backup of the private key.

## Continuous integration

`.github/workflows/ci.yml` runs on every pull request and on pushes to `main`:

- **Lint & build frontend** — oxlint, then the static export.
- **Test Rust backend** — `cargo test`, which compiles and links every target, so a codegen or
  linker error cannot slip through to a release tag.
- **Build installers** — only when a change can actually reach packaging (bundle config, icons,
  capabilities, manifests, dependency set), since rebuilding them for every source edit was not
  paying for itself.

## Available scripts

| Command              | Description                                                      |
| -------------------- | ---------------------------------------------------------------- |
| `pnpm tauri dev`     | Run the desktop app in development mode with hot-reloading       |
| `pnpm run dev`       | Run the Next.js frontend only in the browser (no Tauri commands) |
| `pnpm tauri build`   | Build production installers                                      |
| `pnpm run build`     | Build the static export into `out/`                              |
| `pnpm run lint`      | Lint with oxlint (auto-fix)                                      |
| `pnpm run fmt`       | Format with oxfmt                                                |
| `pnpm run fmt:check` | Report unformatted files without rewriting them                  |

Formatting covers `src/`, plus TypeScript and Markdown anywhere; `src-tauri/` is left to the Rust
toolchain. **Do not run `pnpm run fmt` as part of a feature change** — a repo-wide reformat buried in
a feature diff makes the real change unreviewable. Use `fmt:check` to look without touching.

### Adding UI components

```bash
pnpm dlx shadcn@latest add <component>
```

They are placed under `src/app/components/ui/`.

## Roadmap

- [ ] Analytics route — post performance and traffic (the dashboard has R2/D1 usage today)
- [ ] Series management UI (the backend commands exist; the screens do not)
- [ ] Media library beyond images — video and other assets

## License

Not yet specified.
