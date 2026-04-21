"use client";

import { useState } from "react";
import { CheckCircle2, Plus, Search, Upload } from "lucide-react";
import Link from "next/link";
import { POSTS } from "@/lib/data";
import { StatusDot } from "@/components/StatusDot";
import { StatusPill } from "@/components/StatusPill";

// ─── Types ────────────────────────────────────────────────────────────────────

type FilterId = "all" | "published" | "draft";

type UploadStatus =
  | { kind: "idle" }
  | { kind: "loading" }
  | { kind: "success"; title: string }
  | { kind: "error";   message: string };

export default function PostsPage() {
  const [filter, setFilter]             = useState<FilterId>("all");
  const [search, setSearch]             = useState("");
  const [uploadStatus, setUploadStatus] = useState<UploadStatus>({ kind: "idle" });

  const handleUploadArticle = async () => {
    setUploadStatus({ kind: "loading" });
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      const title = await invoke<string>("upload_article");
      setUploadStatus({ kind: "success", title });
      setTimeout(() => setUploadStatus({ kind: "idle" }), 4000);
    } catch (err) {
      const msg = String(err);
      if (msg === "cancelled") {
        setUploadStatus({ kind: "idle" });
        return;
      }
      setUploadStatus({ kind: "error", message: msg });
      setTimeout(() => setUploadStatus({ kind: "idle" }), 6000);
    }
  };

  const visible = POSTS.filter((p) => {
    const q = search.toLowerCase();
    const matchSearch =
      q === "" ||
      p.title.toLowerCase().includes(q) ||
      p.tags.some((t) => t.includes(q));
    const matchFilter = filter === "all" || p.status === filter;
    return matchSearch && matchFilter;
  });

  const tabs: { id: FilterId; label: string; count: number }[] = [
    { id: "all",       label: "All",       count: POSTS.length },
    { id: "published", label: "Published", count: POSTS.filter((p) => p.status === "published").length },
    { id: "draft",     label: "Drafts",    count: POSTS.filter((p) => p.status === "draft").length },
  ];

  return (
    <main className="flex-1 overflow-y-auto p-6">
      <div className="space-y-4 w-full">
        {/* Toolbar */}
        <div className="flex items-center justify-between gap-4">
          {/* Left: tabs + search */}
          <div className="flex items-center gap-3">
            {/* Segmented tabs */}
            <div className="flex items-center p-[3px] rounded-[7px] bg-zinc-100 dark:bg-white/[0.04] border border-zinc-200 dark:border-white/[0.07] gap-px">
              {tabs.map(({ id, label, count }) => (
                <button
                  key={id}
                  onClick={() => setFilter(id)}
                  className={[
                    "flex items-center gap-1.5 h-[26px] px-3 rounded-[5px] text-[12px] font-semibold",
                    "transition-[background-color,color,box-shadow] duration-150",
                    "active:scale-[0.97] active:transition-none",
                    filter === id
                      ? "bg-white dark:bg-white/[0.1] text-zinc-800 dark:text-zinc-100 shadow-[0_1px_3px_rgba(0,0,0,0.1),0_1px_0_rgba(0,0,0,0.04)] dark:shadow-[0_1px_3px_rgba(0,0,0,0.4)]"
                      : "text-zinc-500 dark:text-zinc-500 hover:text-zinc-700 dark:hover:text-zinc-300",
                  ].join(" ")}
                >
                  {label}
                  <span
                    className={[
                      "text-[10px] font-bold tabular-nums",
                      filter === id
                        ? "text-zinc-400 dark:text-zinc-500"
                        : "text-zinc-400 dark:text-zinc-700",
                    ].join(" ")}
                  >
                    {count}
                  </span>
                </button>
              ))}
            </div>

            {/* Search */}
            <div className="relative">
              <Search
                size={13}
                strokeWidth={1.8}
                className="absolute left-[9px] top-1/2 -translate-y-1/2 text-zinc-400 dark:text-zinc-600 pointer-events-none"
              />
              <input
                type="text"
                placeholder="Search posts, tags…"
                value={search}
                onChange={(e) => setSearch(e.target.value)}
                className={[
                  "h-[30px] w-[200px] pl-[28px] pr-3 text-[12px]",
                  "rounded-[6px] border",
                  "border-zinc-200 dark:border-white/[0.08]",
                  "bg-zinc-50 dark:bg-white/[0.04]",
                  "text-zinc-900 dark:text-zinc-100",
                  "placeholder:text-zinc-400 dark:placeholder:text-zinc-600",
                  "focus:outline-none focus:ring-[1.5px] focus:ring-zinc-400/60 dark:focus:ring-white/[0.2]",
                  "focus:border-zinc-300 dark:focus:border-white/[0.15]",
                  "focus:bg-white dark:focus:bg-white/[0.06]",
                  "transition-[border-color,box-shadow,background-color] duration-150",
                ].join(" ")}
              />
            </div>
          </div>

          {/* Right: CTAs */}
          <div className="flex items-center gap-2 shrink-0">
            {/* Upload Article */}
            <button
              onClick={handleUploadArticle}
              disabled={uploadStatus.kind === "loading"}
              className={[
                "flex items-center gap-[6px] h-[30px] px-3 rounded-[6px]",
                "text-[13px] font-semibold",
                "text-zinc-600 dark:text-zinc-400",
                "border border-zinc-200 dark:border-white/[0.1]",
                "bg-white dark:bg-white/[0.04]",
                "hover:bg-zinc-50 dark:hover:bg-white/[0.07] hover:text-zinc-800 dark:hover:text-zinc-200",
                "hover:border-zinc-300 dark:hover:border-white/[0.16]",
                "active:scale-[0.97] active:translate-y-px active:transition-none",
                "disabled:opacity-50 disabled:cursor-not-allowed disabled:pointer-events-none",
                "transition-[background-color,border-color,color] duration-150",
              ].join(" ")}
            >
              <Upload size={13} strokeWidth={2} />
              {uploadStatus.kind === "loading" ? "Uploading…" : "Upload Article"}
            </button>

            {/* New Post */}
            <Link
              href="/posts/new"
              className={[
                "flex items-center gap-[6px] h-[30px] px-3 rounded-[6px]",
                "text-[13px] font-semibold text-white dark:text-zinc-900",
                "bg-zinc-900 dark:bg-zinc-100",
                "shadow-[0_1px_2px_rgba(0,0,0,0.12)]",
                "hover:bg-zinc-800 dark:hover:bg-white",
                "hover:shadow-[0_2px_8px_rgba(0,0,0,0.18)] dark:hover:shadow-[0_2px_8px_rgba(0,0,0,0.5)]",
                "active:scale-[0.97] active:translate-y-px active:shadow-[0_1px_2px_rgba(0,0,0,0.12)] active:transition-none",
                "transition-all duration-150",
              ].join(" ")}
            >
              <Plus size={13} strokeWidth={2.5} />
              New Post
            </Link>
          </div>
        </div>

        {/* Upload feedback banner */}
        {uploadStatus.kind !== "idle" && uploadStatus.kind !== "loading" && (
          <div
            className={[
              "flex items-center gap-2 px-3 py-2 rounded-[6px] text-[12px] font-medium border",
              uploadStatus.kind === "success"
                ? "bg-emerald-50 dark:bg-emerald-500/[0.08] border-emerald-200 dark:border-emerald-500/[0.2] text-emerald-700 dark:text-emerald-400"
                : "bg-red-50 dark:bg-red-500/[0.08] border-red-200 dark:border-red-500/[0.2] text-red-700 dark:text-red-400",
            ].join(" ")}
          >
            {uploadStatus.kind === "success" ? (
              <>
                <CheckCircle2 size={13} strokeWidth={2} className="shrink-0" />
                <span>
                  <span className="font-semibold">&ldquo;{uploadStatus.title}&rdquo;</span>
                  {" "}uploaded to R2 and registered in D1.
                </span>
              </>
            ) : (
              <>
                <span className="shrink-0 font-bold">Error:</span>
                {uploadStatus.message}
              </>
            )}
          </div>
        )}

        {/* Table */}
        <div className="rounded-[8px] border border-zinc-200 dark:border-white/[0.07] overflow-hidden">
          {/* Head */}
          <div className="grid grid-cols-[1fr_auto_auto_auto] sm:grid-cols-[1fr_120px_90px_100px_80px] gap-0 border-b border-zinc-200 dark:border-white/[0.07] bg-zinc-50 dark:bg-white/[0.02] px-4 py-[8px]">
            {["Title", "Tags", "Status", "Date", "Views"].map((h, i) => (
              <span
                key={h}
                className={[
                  "text-[10px] font-bold uppercase tracking-[0.1em] text-zinc-400 dark:text-zinc-600",
                  i === 4 ? "text-right hidden sm:block" : "",
                  i === 1 ? "hidden sm:block" : "",
                  i === 3 ? "hidden sm:block" : "",
                ].join(" ")}
              >
                {h}
              </span>
            ))}
          </div>

          {/* Rows */}
          <div className="bg-white dark:bg-[#161616] divide-y divide-zinc-100 dark:divide-white/[0.04]">
            {visible.map((post) => (
              <div
                key={post.id}
                className="group grid grid-cols-[1fr_auto_auto_auto] sm:grid-cols-[1fr_120px_90px_100px_80px] items-center gap-0 px-4 py-[10px] cursor-pointer hover:bg-zinc-50 dark:hover:bg-white/[0.02] transition-colors duration-100"
              >
                <div className="flex items-center gap-2.5 min-w-0 pr-4">
                  <StatusDot status={post.status} />
                  <span className="text-[13px] font-medium text-zinc-800 dark:text-zinc-200 truncate group-hover:text-zinc-900 dark:group-hover:text-white transition-colors duration-100">
                    {post.title}
                  </span>
                </div>

                <div className="hidden sm:flex gap-1 flex-wrap">
                  {post.tags.map((t) => (
                    <span
                      key={t}
                      className="inline-block px-[6px] py-[2px] text-[10px] font-mono font-semibold rounded-[4px] bg-zinc-100 dark:bg-white/[0.05] border border-zinc-200 dark:border-white/[0.07] text-zinc-500 dark:text-zinc-500"
                    >
                      {t}
                    </span>
                  ))}
                </div>

                <div>
                  <StatusPill status={post.status} />
                </div>

                <span className="hidden sm:block text-[11px] font-mono tracking-tight text-zinc-400 dark:text-zinc-600">
                  {post.date}
                </span>

                <span className="hidden sm:block text-right text-[12px] font-mono tabular-nums text-zinc-400 dark:text-zinc-600">
                  {post.views !== undefined ? post.views.toLocaleString() : "—"}
                </span>
              </div>
            ))}
          </div>

          {visible.length === 0 && (
            <div className="bg-white dark:bg-[#161616] py-16 text-center">
              <p className="text-[13px] text-zinc-400 dark:text-zinc-600">
                No posts match this filter.
              </p>
            </div>
          )}
        </div>

        {visible.length > 0 && (
          <p className="text-[11px] text-zinc-400 dark:text-zinc-600 px-1">
            {visible.length} of {POSTS.length} posts
          </p>
        )}
      </div>
    </main>
  );
}
