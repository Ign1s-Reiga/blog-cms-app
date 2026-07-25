"use client";

import { useEffect, useRef, useState } from "react";
import { flushSync } from "react-dom";
import { ArrowLeft, Columns2, Eye, PenLine, Tag } from "lucide-react";
import Link from "next/link";
import { renderMarkdown } from "@ign1s-reiga/marked-presets";
import "@ign1s-reiga/marked-presets/styles";
import "./markdown-theme.css";
import { useTheme } from "next-themes";
import { Button } from "@/components/ui/button";
import { Separator } from "@/components/ui/separator";
import { cn } from "@/lib/utils";

type EditorMode = "write" | "split" | "preview";

// ─── Helpers ──────────────────────────────────────────────────────────────────

function wordCount(text: string): number {
  return text.trim() === "" ? 0 : text.trim().split(/\s+/).length;
}

function today(): string {
  return new Date().toISOString().split("T")[0];
}

// One "tab" is four spaces.
const INDENT = "    ";

// Given the current line, returns the Markdown marker to continue on the next
// line (unordered/ordered/task list or blockquote), plus whether the current
// item is empty (in which case Enter should end the list/quote instead). Returns
// null when the line isn't a continuable construct.
function continuationMarker(line: string): { marker: string; isEmpty: boolean } | null {
  // Unordered or task list: -, *, + optionally followed by a [ ] / [x] checkbox.
  let m = line.match(/^(\s*)([-*+])[ \t]+(\[[ xX]\][ \t]+)?(.*)$/);
  if (m) {
    const [, indent, bullet, checkbox, rest] = m;
    const marker = checkbox ? `${indent}${bullet} [ ] ` : `${indent}${bullet} `;
    return { marker, isEmpty: rest.trim() === "" };
  }
  // Ordered list: "1." or "1)" — continue with the next number.
  m = line.match(/^(\s*)(\d+)([.)])[ \t]+(.*)$/);
  if (m) {
    const [, indent, num, delim, rest] = m;
    return { marker: `${indent}${Number(num) + 1}${delim} `, isEmpty: rest.trim() === "" };
  }
  // Blockquote: one or more leading ">" (nesting preserved).
  m = line.match(/^(\s*(?:>[ \t]?)+)(.*)$/);
  if (m) {
    const [, prefix, rest] = m;
    return { marker: prefix, isEmpty: rest.trim() === "" };
  }
  return null;
}

// ─── PostEditor ───────────────────────────────────────────────────────────────

export function PostEditor() {
  const [title, setTitle] = useState("");
  const [tags, setTags]   = useState("");
  const [body, setBody]   = useState("");

  const [mode, setMode] = useState<EditorMode>("write");
  const [preview, setPreview] = useState("");

  // The preview palette is bound to the app theme in markdown-theme.css (the
  // preset's light-dark() colors don't follow next-themes' class — see that
  // file). We still set `color-scheme` on the container so native UI inside the
  // preview (scrollbars, etc.) matches the resolved theme.
  const { resolvedTheme } = useTheme();
  const previewColorScheme = resolvedTheme === "dark" ? "dark" : "light";

  const words = wordCount(body);
  const chars = body.length;

  const textareaRef = useRef<HTMLTextAreaElement>(null);

  // Replace the textarea content and restore the caret. flushSync commits the
  // state update before we move the caret, so the controlled value and the
  // selection stay in sync (React would otherwise reset the caret to the end).
  function applyEdit(nextValue: string, selStart: number, selEnd: number = selStart) {
    flushSync(() => setBody(nextValue));
    const el = textareaRef.current;
    if (el) {
      el.focus();
      el.setSelectionRange(selStart, selEnd);
    }
  }

  function handleEditorKeyDown(e: React.KeyboardEvent<HTMLTextAreaElement>) {
    if (e.nativeEvent.isComposing) return; // don't interfere with IME
    const { value, selectionStart: start, selectionEnd: end } = e.currentTarget;

    // Tab → insert four spaces (indent selected lines when there's a selection).
    if (e.key === "Tab" && !e.shiftKey && !e.ctrlKey && !e.metaKey && !e.altKey) {
      e.preventDefault();
      if (start === end) {
        applyEdit(value.slice(0, start) + INDENT + value.slice(end), start + INDENT.length);
      } else {
        const lineStart = value.lastIndexOf("\n", start - 1) + 1;
        const block = value.slice(lineStart, end);
        const indented = block.replace(/^/gm, INDENT);
        const next = value.slice(0, lineStart) + indented + value.slice(end);
        applyEdit(next, start + INDENT.length, end + (indented.length - block.length));
      }
      return;
    }

    // Enter → continue the current list item / blockquote automatically.
    if (e.key === "Enter" && !e.shiftKey && !e.ctrlKey && !e.metaKey && !e.altKey && start === end) {
      const lineStart = value.lastIndexOf("\n", start - 1) + 1;
      let lineEnd = value.indexOf("\n", start);
      if (lineEnd === -1) lineEnd = value.length;
      const cont = continuationMarker(value.slice(lineStart, lineEnd));
      if (!cont) return; // ordinary newline
      e.preventDefault();
      if (cont.isEmpty) {
        // Empty item: drop the marker and end the list/quote.
        applyEdit(value.slice(0, lineStart) + value.slice(lineEnd), lineStart);
      } else {
        const insertion = "\n" + cont.marker;
        applyEdit(value.slice(0, start) + insertion + value.slice(start), start + insertion.length);
      }
    }
  }

  // Render the Markdown body to HTML for the preview pane. Runs live in both
  // split and preview modes, debounced so we don't re-parse on every keystroke.
  // renderMarkdown is async (Shiki is created lazily on first call); a
  // cancellation flag guards against out-of-order resolves, and the previous
  // HTML stays on screen while the next render is in flight so typing in split
  // mode doesn't flash.
  useEffect(() => {
    if (mode === "write") return;
    if (body.trim() === "") {
      setPreview("");
      return;
    }

    let cancelled = false;
    const timer = setTimeout(() => {
      renderMarkdown(body)
        .then((html) => { if (!cancelled) setPreview(html); })
        .catch(() => { if (!cancelled) setPreview('<p class="md-error">Failed to render preview.</p>'); });
    }, 200);

    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [mode, body]);

  // ── Shared fields, composed differently per layout mode below ────────────────

  const titleField = (
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
  );

  const tagsField = (
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
  );

  const divider = (
    <Separator className="bg-zinc-100 dark:bg-white/[0.04] mx-4 mb-2 w-[calc(100%-2rem)]" />
  );

  const editor = (
    <textarea
      ref={textareaRef}
      value={body}
      onChange={(e) => setBody(e.target.value)}
      onKeyDown={handleEditorKeyDown}
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
  );

  const previewBody =
    body.trim() === "" ? (
      <p className="font-mono text-[13px] text-zinc-300 dark:text-zinc-700">
        Nothing to preview yet.
      </p>
    ) : preview === "" ? (
      <p className="font-mono text-[13px] text-zinc-300 dark:text-zinc-700">
        Rendering…
      </p>
    ) : (
      // Intentionally unsanitized: `body` is the author's own Markdown, rendered
      // locally in a single-user preview (self-XSS only), and the preset relies
      // on injecting its own HTML (inline copy-button handler, SVG alert icons).
      // SANITIZE INSTEAD AT THE PUBLISH/READ PATH — where post HTML is served to
      // other readers or synced from an untrusted source — ideally server-side
      // in the Rust backend. See memory: project_markdown_sanitize_at_publish.
      <div
        className="md-body text-[14px]"
        style={{ colorScheme: previewColorScheme }}
        dangerouslySetInnerHTML={{ __html: preview }}
      />
    );

  return (
    <div className="flex flex-col flex-1 min-h-0 bg-white dark:bg-[#161616]">

      {/* ── Topbar ──────────────────────────────────────────────────────── */}
      <div className="relative flex items-center justify-between px-5 h-[48px] shrink-0 border-b border-zinc-200 dark:border-white/[0.06]">
        {/* Back */}
        <Button
          asChild
          variant="ghost"
          size="sm"
          className="h-[28px] px-2 -ml-1 gap-1.5 rounded-[5px] text-[12px] font-medium text-zinc-500 dark:text-zinc-400"
        >
          <Link href="/posts">
            <ArrowLeft size={13} strokeWidth={2} />
            Back to Posts
          </Link>
        </Button>

        {/* Write / Split / Preview toggle */}
        <div className="absolute left-1/2 -translate-x-1/2 flex items-center gap-0.5 p-[3px] rounded-[7px] bg-zinc-100 dark:bg-white/[0.04]">
          {([
            { value: "write",   label: "Write",   Icon: PenLine },
            { value: "split",   label: "Split",   Icon: Columns2 },
            { value: "preview", label: "Preview", Icon: Eye },
          ] as const).map(({ value, label, Icon }) => (
            <button
              key={value}
              type="button"
              onClick={() => setMode(value)}
              aria-pressed={mode === value}
              className={cn(
                "flex items-center gap-1.5 h-[22px] px-2.5 rounded-[5px] text-[12px] font-medium transition-colors active:scale-95",
                mode === value
                  ? "bg-white dark:bg-white/[0.08] text-zinc-900 dark:text-zinc-100 shadow-[0_1px_2px_rgba(0,0,0,0.08)]"
                  : "text-zinc-500 dark:text-zinc-400 hover:text-zinc-800 dark:hover:text-zinc-200",
              )}
            >
              <Icon size={12} strokeWidth={2} />
              {label}
            </button>
          ))}
        </div>

        {/* Actions */}
        <div className="flex items-center gap-2">
          <Button
            variant="outline"
            size="sm"
            className="h-[28px] px-3 rounded-[5px] text-[12px] font-semibold text-zinc-600 dark:text-zinc-400"
          >
            Save Draft
          </Button>

          <Button
            size="sm"
            className="h-[28px] px-3 rounded-[5px] text-[12px] font-semibold shadow-[0_1px_2px_rgba(0,0,0,0.12)] hover:shadow-[0_2px_8px_rgba(0,0,0,0.18)] dark:hover:shadow-[0_2px_8px_rgba(0,0,0,0.5)]"
          >
            Publish
          </Button>
        </div>
      </div>

      {/* ── Document area ───────────────────────────────────────────────── */}
      {mode === "split" ? (
        /* VS Code-style split: Markdown source (left) + live preview (right). */
        <div className="flex-1 min-h-0 flex flex-col w-full">
          <div className="shrink-0 w-full">
            {titleField}
            {tagsField}
          </div>
          {divider}
          <div className="flex-1 min-h-0 flex">
            <div className="flex-1 min-w-0 min-h-0 flex flex-col overflow-hidden">
              {editor}
            </div>
            <div className="flex-1 min-w-0 min-h-0 overflow-y-auto px-5 py-3 border-l border-zinc-200 dark:border-white/[0.06]">
              {previewBody}
            </div>
          </div>
        </div>
      ) : (
        <div className="flex-1 min-h-0 flex flex-col max-w-[760px] w-full mx-auto px-2">
          {titleField}
          {tagsField}
          {divider}
          {mode === "write" ? (
            editor
          ) : (
            <div className="flex-1 min-h-0 w-full overflow-y-auto px-4 py-3">
              {previewBody}
            </div>
          )}
        </div>
      )}

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
