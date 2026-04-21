"use client";

import { useState } from "react";
import { ArrowLeft, Tag } from "lucide-react";
import Link from "next/link";

// ─── Helpers ──────────────────────────────────────────────────────────────────

function wordCount(text: string): number {
  return text.trim() === "" ? 0 : text.trim().split(/\s+/).length;
}

function today(): string {
  return new Date().toISOString().split("T")[0];
}

// ─── PostEditor ───────────────────────────────────────────────────────────────

export function PostEditor() {
  const [title, setTitle] = useState("");
  const [tags, setTags]   = useState("");
  const [body, setBody]   = useState("");

  const words = wordCount(body);
  const chars = body.length;

  return (
    <div className="flex flex-col flex-1 min-h-0 bg-white dark:bg-[#161616]">

      {/* ── Topbar ──────────────────────────────────────────────────────── */}
      <div className="flex items-center justify-between px-5 h-[48px] shrink-0 border-b border-zinc-200 dark:border-white/[0.06]">
        {/* Back */}
        <Link
          href="/posts"
          className={[
            "flex items-center gap-1.5 h-[28px] px-2 -ml-1 rounded-[5px]",
            "text-[12px] font-medium text-zinc-500 dark:text-zinc-400",
            "hover:bg-zinc-100 dark:hover:bg-white/[0.06] hover:text-zinc-800 dark:hover:text-zinc-200",
            "active:scale-[0.97] active:transition-none",
            "transition-[background-color,color] duration-100",
          ].join(" ")}
        >
          <ArrowLeft size={13} strokeWidth={2} />
          Back to Posts
        </Link>

        {/* Actions */}
        <div className="flex items-center gap-2">
          <button
            className={[
              "flex items-center gap-1.5 h-[28px] px-3 rounded-[5px]",
              "text-[12px] font-semibold",
              "text-zinc-600 dark:text-zinc-400",
              "border border-zinc-200 dark:border-white/[0.1]",
              "bg-white dark:bg-white/[0.04]",
              "hover:bg-zinc-50 dark:hover:bg-white/[0.07] hover:text-zinc-800 dark:hover:text-zinc-200",
              "hover:border-zinc-300 dark:hover:border-white/[0.16]",
              "active:scale-[0.97] active:translate-y-px active:transition-none",
              "transition-[background-color,border-color,color] duration-150",
            ].join(" ")}
          >
            Save Draft
          </button>

          <button
            className={[
              "flex items-center gap-1.5 h-[28px] px-3 rounded-[5px]",
              "text-[12px] font-semibold",
              "bg-zinc-900 dark:bg-zinc-100 text-white dark:text-zinc-900",
              "shadow-[0_1px_2px_rgba(0,0,0,0.12)]",
              "hover:bg-zinc-800 dark:hover:bg-white",
              "hover:shadow-[0_2px_8px_rgba(0,0,0,0.18)] dark:hover:shadow-[0_2px_8px_rgba(0,0,0,0.5)]",
              "active:scale-[0.97] active:translate-y-px active:shadow-[0_1px_2px_rgba(0,0,0,0.12)] active:transition-none",
              "transition-all duration-150",
            ].join(" ")}
          >
            Publish
          </button>
        </div>
      </div>

      {/* ── Document area ───────────────────────────────────────────────── */}
      <div className="flex-1 min-h-0 flex flex-col max-w-[760px] w-full mx-auto px-2">

        <input
          type="text"
          value={title}
          onChange={(e) => setTitle(e.target.value)}
          placeholder="Post title"
          className={[
            "w-full px-4 pt-8 pb-1",
            "text-[26px] font-bold leading-snug tracking-tight",
            "text-zinc-900 dark:text-zinc-50",
            "placeholder:text-zinc-200 dark:placeholder:text-zinc-800",
            "bg-transparent border-none outline-none focus:ring-0",
            "shrink-0",
          ].join(" ")}
        />

        <div className="flex items-center gap-2 px-4 py-2 shrink-0">
          <Tag size={11} strokeWidth={1.8} className="text-zinc-300 dark:text-zinc-700 shrink-0" />
          <input
            type="text"
            value={tags}
            onChange={(e) => setTags(e.target.value)}
            placeholder="Tags (comma-separated)"
            className={[
              "flex-1 text-[12px] font-medium",
              "text-zinc-500 dark:text-zinc-500",
              "placeholder:text-zinc-300 dark:placeholder:text-zinc-700",
              "bg-transparent border-none outline-none focus:ring-0",
            ].join(" ")}
          />
          <span className="text-zinc-200 dark:text-zinc-800 shrink-0">·</span>
          <span className="text-[11px] font-mono tracking-tight text-zinc-300 dark:text-zinc-700 shrink-0">
            {today()}
          </span>
        </div>

        <div className="h-px bg-zinc-100 dark:bg-white/[0.04] mx-4 mb-2 shrink-0" />

        <textarea
          value={body}
          onChange={(e) => setBody(e.target.value)}
          placeholder={`Start writing in Markdown…\n\n## Heading\n\nYour content here.`}
          spellCheck
          className={[
            "flex-1 min-h-0 w-full resize-none",
            "px-4 py-3",
            "font-mono text-[13.5px] leading-[1.85]",
            "text-zinc-700 dark:text-zinc-300",
            "placeholder:text-zinc-300 dark:placeholder:text-zinc-700",
            "bg-transparent border-none outline-none focus:ring-0",
            "overflow-y-auto",
          ].join(" ")}
        />
      </div>

      {/* ── Status bar ──────────────────────────────────────────────────── */}
      <div className="flex items-center justify-between px-5 h-[34px] shrink-0 border-t border-zinc-100 dark:border-white/[0.04]">
        <div className="flex items-center gap-3">
          <span className="text-[11px] font-mono text-zinc-300 dark:text-zinc-700">
            {words.toLocaleString()} {words === 1 ? "word" : "words"}
          </span>
          <span className="text-zinc-200 dark:text-zinc-800">·</span>
          <span className="text-[11px] font-mono text-zinc-300 dark:text-zinc-700">
            {chars.toLocaleString()} {chars === 1 ? "character" : "characters"}
          </span>
        </div>
        <span className="text-[10px] font-bold uppercase tracking-[0.1em] text-zinc-300 dark:text-zinc-700">
          Markdown
        </span>
      </div>
    </div>
  );
}
