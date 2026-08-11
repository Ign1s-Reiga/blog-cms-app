# blog-cms-app

A cross-platform **desktop CMS for blog content**, built with [Tauri](https://tauri.app) and [Next.js](https://nextjs.org). Write in Markdown locally and sync your posts, assets, and metadata to Cloudflare — **R2** for raw files and media, **D1** for fast, queryable metadata.

> **Status — early development (v0.1.0).** The dashboard, posts list, and Markdown editor UI are in place. The first end-to-end sync command (`upload_article`) uploads a Markdown file to R2 and registers its metadata in D1. Media, Analytics, and Settings are placeholder screens for now (see [Roadmap](#roadmap)).

---

## Features

- **Modern desktop UI** — a "SaaS console" layout (fixed sidebar + header + content area) inspired by Vercel, Supabase, and Linear, with full **dark/light** theming.
- **Markdown-first editor** — a distraction-free writing surface with live word and character counts.
- **Post management** — a searchable, filterable posts table (All / Published / Drafts) with status pills and tags.
- **One-step Cloudflare sync** — pick a `.md` file and it is uploaded to **R2** and registered in **D1** in a single action. YAML front-matter (`title`, `tags`) is parsed on the Rust side.
- **In-app updates** — checks GitHub Releases on launch and installs signed updates from `Settings → Software update`.
- **Built with [shadcn/ui](https://ui.shadcn.com)** — accessible Radix-based primitives (Button, Tabs, Badge, Input, Card, Alert, Avatar, Breadcrumb, Separator) themed to the design system.

## Tech stack

| Layer            | Technology                                                        |
| ---------------- | ----------------------------------------------------------------- |
| Desktop shell    | Tauri 2 (Rust)                                                    |
| Frontend         | Next.js 16 (App Router) · React 19                                |
| Styling          | Tailwind CSS v4 · shadcn/ui · lucide-react                        |
| Theming          | next-themes (dark / light)                                        |
| Backend (Rust)   | reqwest (Cloudflare REST API) · tokio · uuid · chrono            |
| Cloud            | Cloudflare R2 (files & media) · Cloudflare D1 (metadata)          |
| Tooling          | pnpm · oxlint · oxfmt                                             |

## Project structure

```text
blog-cms-app/
├── src/
│   └── app/                    # Next.js App Router (frontend)
│       ├── components/         # UI components
│       │   └── ui/             # shadcn/ui primitives
│       ├── lib/                # cn() util, mock data
│       ├── posts/              # /posts and /posts/new routes
│       ├── media/ analytics/ settings/   # additional routes
│       ├── layout.tsx          # root layout, theme + sidebar providers
│       ├── page.tsx            # dashboard
│       └── globals.css         # Tailwind import + theme tokens
├── src-tauri/                  # Tauri backend (Rust)
│   ├── src/
│   │   ├── commands.rs         # #[tauri::command] handlers (upload_article)
│   │   ├── cloudflare.rs       # R2 upload + D1 insert
│   │   └── lib.rs / main.rs    # app entry point
│   ├── capabilities/           # Tauri permission definitions
│   └── tauri.conf.json         # Tauri configuration
├── components.json             # shadcn/ui configuration
├── next.config.ts
└── package.json
```

## Prerequisites

- **Node.js 20.9+** and **[pnpm](https://pnpm.io)**
- **Rust** toolchain (1.77+) — install via [rustup](https://rustup.rs)
- Tauri OS dependencies — see the [Tauri prerequisites guide](https://tauri.app/start/prerequisites/) (e.g. **WebView2** and MSVC build tools on Windows)

## Getting started

### 1. Install dependencies

```bash
pnpm install
```

### 2. Configure Cloudflare credentials

The sync command reads credentials from environment variables **at call time**, so they must be present in the shell that launches the app. See [Cloudflare configuration](#cloudflare-configuration) below for details.

```bash
# macOS / Linux
export CF_ACCOUNT_ID="your-account-id"
export CF_API_TOKEN="your-api-token"
export CF_R2_BUCKET="your-bucket-name"
export CF_D1_DATABASE_ID="your-d1-database-id"
```

```powershell
# Windows (PowerShell)
$env:CF_ACCOUNT_ID     = "your-account-id"
$env:CF_API_TOKEN      = "your-api-token"
$env:CF_R2_BUCKET      = "your-bucket-name"
$env:CF_D1_DATABASE_ID = "your-d1-database-id"
```

### 3. Run the app

```bash
pnpm tauri dev
```

This launches the full desktop app with hot-reloading. To work on the frontend alone in a browser (Tauri commands such as upload will be unavailable), run `pnpm run dev` and open <http://localhost:3000>.

## Cloudflare configuration

### Environment variables

| Variable            | Description                                             |
| ------------------- | ------------------------------------------------------- |
| `CF_ACCOUNT_ID`     | Your Cloudflare account ID                              |
| `CF_API_TOKEN`      | API token with **R2 Edit** and **D1 Edit** permissions  |
| `CF_R2_BUCKET`      | Target R2 bucket name                                   |
| `CF_D1_DATABASE_ID` | D1 database ID (UUID from the dashboard)                |

### D1 schema

The `upload_article` command inserts into a `posts` table with the following schema:

```sql
CREATE TABLE posts (
  id                TEXT PRIMARY KEY,
  title             TEXT NOT NULL,
  upload_date       TEXT NOT NULL,
  last_updated_date TEXT NOT NULL,
  tags              TEXT NOT NULL DEFAULT ''
);
```

## How sync works

When you upload an article, the Rust backend:

1. Opens a native file picker (`tauri-plugin-dialog`) filtered to Markdown files.
2. Reads the file and parses its YAML front-matter for `title` and `tags` (falling back to the file name for the title).
3. Uploads the raw Markdown to **R2** under `posts/<uuid>.md`.
4. On success, inserts the post's metadata into **D1**.

The frontend surfaces success and error states (and treats a cancelled dialog as a no-op).

## Releases & auto-update

The app updates itself from this repository's **GitHub Releases**.

**How it works.** Pushing a `v*` tag runs `.github/workflows/release.yml`, which builds the
Windows installers, signs the updater bundle, and attaches a `latest.json` manifest to a **draft**
release. Installed apps poll `releases/latest/download/latest.json`, compare its version against
their own, and verify the bundle's minisign signature before installing — so an update is only
offered once you **publish** the draft.

In the app, `Settings → Software update` shows the running version and drives
check → download → install → restart; the sidebar surfaces a notice when a check finds
a newer version. Checks run once per launch and are cached for the session.

**One-time setup.** The workflow signs updates with a minisign key that must not live in the repo.
Generate one and add it to the repository secrets:

```bash
pnpm tauri signer generate -w ~/.tauri/blog-cms-app.key
```

| Secret                               | Value                            |
| ------------------------------------ | -------------------------------- |
| `TAURI_SIGNING_PRIVATE_KEY`          | Contents of the private key file |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | The passphrase you chose         |

Give the key a real passphrase — GitHub rejects empty secret values, so a passphrase-less key
has nothing valid to put in the second secret.

The matching **public** key lives in `src-tauri/tauri.conf.json` under `plugins.updater.pubkey`.
The two must stay paired: replacing the key means already-installed apps can no longer verify
updates and have to be reinstalled manually. Keep a backup of the private key.

## Available scripts

| Command            | Description                                                          |
| ------------------ | ------------------------------------------------------------------- |
| `pnpm tauri dev`   | Run the desktop app in development mode with hot-reloading           |
| `pnpm run dev`     | Run the Next.js frontend only in the browser (limited Tauri access) |
| `pnpm tauri build` | Build production installers (MSI, DMG, AppImage, …)                  |
| `pnpm run lint`    | Lint with oxlint (auto-fix)                                          |
| `pnpm run fmt`     | Format with oxfmt                                                    |

### Adding UI components

New shadcn/ui components can be added with:

```bash
pnpm dlx shadcn@latest add <component>
```

They are placed under `src/app/components/ui/`.

## Roadmap

- [ ] Media library backed by R2 (currently a placeholder)
- [ ] Analytics dashboard (currently a placeholder)
- [ ] Settings screen for credentials and sync preferences
- [ ] Local offline cache (SQLite) for drafts before syncing to the cloud
- [ ] Read/update/delete posts from D1 (only create is implemented today)

## License

Not yet specified.
