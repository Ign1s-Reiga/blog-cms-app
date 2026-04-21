# CLAUDE.md

## What is this project?

A desktop application built with **Tauri** and **Next.js** designed for managing blog content (Markdown, assets, metadata) stored on various cloud services.

## Project structure

```text
├─────── src/
│         ├── app/              # Frontend (Next.js AppRouter)
│         │    ├── components/  # UI components
│         │    ├── page.tsx     # Main dashboard
│         │    └── layout.tsx   # Root layout & providers
│         └─── public/
├─────── src-tauri/             # Backend (Rust)
│       ├── src/                # Rust logic & Command handlers
│       └── tauri.conf.json     # Tauri configuration
├── .gitignore
├── README.md
├── tsconfig.json
├── postcss.config.ts
└── next.config.ts
```

## Tech stack

* **Frontend:** Next.js
* **Backend:** Tauri
* **Styling:** Tailwind CSS
* **ORM:** Sea ORM (Rust)
* **Cloud Infrastructure:** Cloudflare (R2, D1)

## UI Design Standards  

### Visual Style "Modern Saas Console"

* **Aesthetic:** Minimalist, flat design inspired by Vercel, Supabase, and Linear.
* **Themes:** Full Dark/Light mode support
  * **Light:** `bg-slate-50` background with `border-slate-200` dividers.
  * **Dark:** `bg-zinc-950` background with `border-zinc-800` dividers.
* **Surface:** Avoid heavy shadows. Use subtle borders (`1px`) to define sections and cards.
* **Typography:** Sans-serif (Inter/Geist). Use `font-medium` for headings and `text-muted-foreground` for secondary text.

### Layout

* **Sidebar:** For navigation. Fixed at left side.
  * Navigation items (Dashboard, Posts, Media, Analytics, Settings) whose background color changes when hovered over.
* **Header:** Fixed to the top of the page. It displays the current page title, search bar, and **theme switching button (Sun/Moon icon)**.
* **Main Content Area:** Occupies the central space, displaying the content for the selected navigation item. Ensure sufficient padding throughout.

### Interactions

* All buttons and links are treated with `transition-colors`.
* Add a slight response to clicks (e.g., active:scale-95).

## Data Management & Architecture

### Storage Strategy

* **Cloudflare R2:** Acts as the primary storage for raw `.md` files and media assets (images/videos)
* **Cloudflare D1:** Stores relational metadata (post status, tags, published dates, and R2 object keys) for fast querying and filtering.

### Integration Flow

* **Direct API/SDK:** The Rust backend (`src-tauri`) handles authentication and communication with Cloudflare via the Cloudflare API or SDK.
* **Sync Logic:** When a post is saved:
  1. Upload/Update the file in **R2**.
  2. Upon **R2** success, upsert the corresponding record in **D1**.
* **Local Caching:** Use a local SQLite or `tauri-plugin-store` for offline editing states before syncing to the cloud.

## Common Commands

### Development

* `pnpm tauri dev`: Run the desktop app in development mode with hot-reloading.
* `pnpm run dev`: Run the Next.js frontend in the browser (limited Tauri functionality).

### Build

* `pnpm tauri build`: Generate production-ready installers (MSI, AppImage, DMG, etc.).

### Quality Control

* `pnpm run lint`: Run Oxlint to check code quality.

## Coding Standards

### Architecture Principles

* **Frontend:** Keep components modular. Use `use client` strictly for components requiring Tauri APIs or React hooks.
* **Backend (Rust):** Implement heavy computations, file system operations, and direct cloud SDK integrations in the Rust core. Expose them to the frontend via `#[tauri::command]`.
* **Security:** Always validate file paths and cloud credentials on the Rust side; never trust raw input from the frontend for OS-level operations.

## Tips

* When adding new features, check if a Tauri plugin (e.g., `fs`, `shell`, `dialog`) is needed before writing custom Rust code.
* Prefer `Lucide React` for icons to maintain a consistent UI language.
* When import components, use absolute imports (e.g., `@/components/SectionHeader`) for better readability and maintainability.
