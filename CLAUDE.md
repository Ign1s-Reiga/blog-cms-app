# CLAUDE.md

## What is this project?

A desktop application built with **Tauri** and **Next.js** designed for managing blog content (Markdown, assets, metadata) stored on various cloud services.

## Project structure

```text
├─────── src/
│         ├── app/              # Frontend (Next.js App Router)
│         │    ├── components/  # UI components
│         │    │    └── ui/     # shadcn/ui primitives (Button, Card, Tabs, …)
│         │    ├── lib/         # Shared utilities (e.g. cn())
│         │    ├── posts/       # Posts routes (list + new)
│         │    ├── media/       # Media route
│         │    ├── analytics/   # Analytics route
│         │    ├── settings/    # Settings route
│         │    ├── page.tsx     # Main dashboard
│         │    ├── layout.tsx   # Root layout & providers
│         │    └── globals.css  # Tailwind v4 theme tokens
│         └─── public/
├─────── src-tauri/             # Backend (Rust)
│       ├── src/                # Rust logic & Command handlers
│       └── tauri.conf.json     # Tauri configuration
├── .gitignore
├── README.md
├── components.json             # shadcn/ui configuration
├── tsconfig.json
├── postcss.config.ts
└── next.config.ts
```

## Tech stack

- **Frontend:** Next.js 16 (App Router) + React 19
- **UI:** shadcn/ui (`radix-nova` style) built on Radix UI primitives
- **Backend:** Tauri 2
- **Styling:** Tailwind CSS v4 (CSS-first config in `globals.css`)
- **Theming:** `next-themes` (class strategy, light/dark)
- **ORM:** Sea ORM (Rust)
- **Cloud Infrastructure:** Cloudflare (R2, D1)

## UI Design Standards

### Visual Style "Modern Saas Console"

- **Aesthetic:** Minimalist, flat design inspired by Vercel, Supabase, and Linear.
- **Themes:** Full Dark/Light mode support
  - **Light:** `bg-slate-50` background with `border-slate-200` dividers.
  - **Dark:** `bg-zinc-950` background with `border-zinc-800` dividers.
- **Surface:** Avoid heavy shadows. Use subtle borders (`1px`) to define sections and cards.
- **Typography:** Sans-serif (Inter/Geist). Use `font-medium` for headings and `text-muted-foreground` for secondary text.

### Layout

- **Sidebar:** For navigation. Fixed at left side.
  - Navigation items (Dashboard, Posts, Media, Analytics, Settings) whose background color changes when hovered over.
- **Header:** Fixed to the top of the page. It displays the current page title, search bar, and **theme switching button (Sun/Moon icon)**.
- **Main Content Area:** Occupies the central space, displaying the content for the selected navigation item. Ensure sufficient padding throughout.

### Interactions

- All buttons and links are treated with `transition-colors`.
- Add a slight response to clicks (e.g., active:scale-95).

### Components

- Build UI from the **shadcn/ui** primitives in `@/components/ui` (Button, Card, Tabs, Badge, Input, Avatar, Alert, Separator, Breadcrumb) before hand-rolling markup.
- Add new primitives with `pnpm dlx shadcn@latest add <component>`; they install into `src/app/components/ui/`.
- Compose class names with the `cn()` helper from `@/lib/utils` (clsx + `tailwind-merge`) so variant and override classes merge cleanly.

## Data Management & Architecture

### Storage Strategy

- **Cloudflare R2:** Acts as the primary storage for raw `.md` files and media assets (images/videos)
- **Cloudflare D1:** Stores relational metadata (post status, tags, published dates, and R2 object keys) for fast querying and filtering.

### Integration Flow

- **Direct API/SDK:** The Rust backend (`src-tauri`) handles authentication and communication with Cloudflare via the Cloudflare API or SDK.
- **Sync Logic:** When a post is saved:
  1. Upload/Update the file in **R2**.
  2. Upon **R2** success, upsert the corresponding record in **D1**.
- **Local Caching:** Use a local SQLite or `tauri-plugin-store` for offline editing states before syncing to the cloud.

## Code Review

Codex reviews pull requests in this repository automatically — on open and on
"ready for review". **A push to an open PR does not trigger a review**, so a
follow-up commit answering review findings is not reviewed by the act of pushing
it. Only an `@codex review` comment or another ready-for-review transition asks
for a second pass.

**Do not post `@codex review` comments** — not because a review is coming anyway,
but for the reason below: the PR conversation belongs to the repository owner.
Push the fix, then say plainly that the new commit is unreviewed and that
triggering another pass is theirs to do. Waiting silently for a review that a
push will not summon is the failure this paragraph exists to prevent.

**Do not comment on pull requests at all.** The repository owner writes every
comment on a PR, including the replies to review findings. This is not about
who is right — it is that the PR conversation is theirs, and an agent posting
into it puts words in the discussion they are meant to be holding.

Answer findings in the work instead. Fix what is real, and put the reasoning in
the commit message: what was wrong, why the fix is the right shape, and — when a
finding does not hold — what the evidence is that it does not. The commit is a
better home for it anyway, since it stays attached to the change after the PR is
closed. Report the same summary to the person you are working with, and let them
decide what goes on the PR.

## Common Commands

### Development

- `pnpm tauri dev`: Run the desktop app in development mode with hot-reloading.
- `pnpm run dev`: Run the Next.js frontend in the browser (limited Tauri functionality).

### Build

- `pnpm tauri build`: Generate production-ready Windows installers (NSIS `.exe` and MSI).

### Quality Control

- `pnpm run lint`: Run Oxlint (with `--fix`) to check and auto-fix code quality.
- `pnpm run fmt`: Format the code with oxfmt.
- `pnpm run fmt:check`: Report unformatted files without rewriting them.

Formatting is scoped to `src/`, plus TypeScript and Markdown anywhere. It leaves
`src-tauri/` to the Rust toolchain, and does not touch workflow YAML or JSON
config such as `tauri.conf.json` and `tsconfig.json`.

**Never run `pnpm run fmt` as part of a feature change.** A repo-wide reformat
buried in a feature diff makes the real change unreviewable — match the style of
the file you are editing instead, and let formatting land in its own commit or
its own PR. To check formatting without rewriting anything, use
`pnpm run fmt:check`.

## Coding Standards

### Architecture Principles

- **Frontend:** Keep components modular. Use `use client` strictly for components requiring Tauri APIs or React hooks.
- **Backend (Rust):** Implement heavy computations, file system operations, and direct cloud SDK integrations in the Rust core. Expose them to the frontend via `#[tauri::command]`.
- **Security:** Always validate file paths and cloud credentials on the Rust side; never trust raw input from the frontend for OS-level operations.

## Tips

- When adding new features, check if a Tauri plugin (e.g., `fs`, `shell`, `dialog`) is needed before writing custom Rust code.
- Prefer `Lucide React` for icons to maintain a consistent UI language.
- When importing components, use absolute imports (e.g., `@/components/SectionHeader`) for better readability and maintainability. The `@/*` alias maps to `src/app/*`.
