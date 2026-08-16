'use client';

import { Fragment, useCallback, useEffect, useRef, useState } from 'react';
import { createPortal, flushSync } from 'react-dom';
import {
  ArrowLeft,
  Bold,
  Columns2,
  Eye,
  History,
  ImagePlus,
  Italic,
  Link2,
  PenLine,
  Strikethrough,
  Tag,
  Underline,
  type LucideIcon,
} from 'lucide-react';
import Link from 'next/link';
import { renderMarkdown } from '@ign1s-reiga/marked-presets';
import '@ign1s-reiga/marked-presets/styles';
import './markdown-theme.css';
import { useTheme } from 'next-themes';
import { convertFileSrc, invoke, isTauri } from '@tauri-apps/api/core';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { appDataDir, join } from '@tauri-apps/api/path';
import { Button } from '@/components/ui/button';
import { StatusPill } from '@/components/StatusPill';
import { MediaPicker, type MediaEntry } from '@/components/MediaPicker';
import { RevisionHistory } from '@/components/RevisionHistory';
import { Separator } from '@/components/ui/separator';
import { cn } from '@/lib/utils';

type EditorMode = 'write' | 'split' | 'preview';

// ─── Helpers ──────────────────────────────────────────────────────────────────

function wordCount(text: string): number {
  return text.trim() === '' ? 0 : text.trim().split(/\s+/).length;
}

function today(): string {
  return new Date().toISOString().split('T')[0];
}

// One "tab" is four spaces.
const INDENT = '    ';

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
    return { marker, isEmpty: rest.trim() === '' };
  }
  // Ordered list: "1." or "1)" — continue with the next number.
  m = line.match(/^(\s*)(\d+)([.)])[ \t]+(.*)$/);
  if (m) {
    const [, indent, num, delim, rest] = m;
    return { marker: `${indent}${Number(num) + 1}${delim} `, isEmpty: rest.trim() === '' };
  }
  // Blockquote: one or more leading ">" (nesting preserved).
  m = line.match(/^(\s*(?:>[ \t]?)+)(.*)$/);
  if (m) {
    const [, prefix, rest] = m;
    return { marker: prefix, isEmpty: rest.trim() === '' };
  }
  return null;
}

// True when the line is a list item or blockquote — the constructs that Tab /
// Shift+Tab indent and outdent by a whole level.
function isListOrQuote(line: string): boolean {
  return continuationMarker(line) !== null;
}

// Indent a single line by one level: a blockquote gains another ">" level,
// anything else gains one INDENT of leading space.
function indentLine(line: string): string {
  const bq = line.match(/^(\s*)((?:>[ \t]?)+)(.*)$/);
  if (bq) {
    const [, lead, marks, rest] = bq;
    return `${lead}> ${marks}${rest}`;
  }
  return INDENT + line;
}

// Outdent a single line by one level: a blockquote drops one ">" level,
// anything else loses up to one INDENT of leading space (or a leading tab).
function outdentLine(line: string): string {
  const bq = line.match(/^(\s*)((?:>[ \t]?)+)(.*)$/);
  if (bq) {
    const [, lead, marks, rest] = bq;
    return `${lead}${marks.replace(/^>[ \t]?/, '')}${rest}`;
  }
  return line.replace(/^(?:\t| {1,4})/, '');
}

// Image files accepted by drag-and-drop (mirrors the Rust allow-list).
const IMAGE_EXT = /\.(?:png|jpe?g|gif|webp|avif|svg|bmp|ico)$/i;

// Shape returned by the `stage_image` Tauri command.
type StagedImage = { rel: string; name: string };

// Rewrite the relative `assets/…` image sources produced when an image is
// dropped into the editor to asset-protocol URLs the webview can actually load
// (the raw relative path would resolve against the dev server). In a browser
// (dev) build there's no asset protocol, so the HTML is returned unchanged.
async function resolveAssetSrcs(html: string): Promise<string> {
  if (!isTauri()) return html;
  const refs = new Set([...html.matchAll(/src="(assets\/[^"]+)"/g)].map((m) => m[1]));
  if (refs.size === 0) return html;
  const base = await appDataDir();
  let out = html;
  for (const ref of refs) {
    const url = convertFileSrc(await join(base, ref));
    out = out.replaceAll(`src="${ref}"`, `src="${url}"`);
  }
  return out;
}

// Format a post's JSON `tags` column (e.g. `["a","b"]`) as the comma-separated
// string the tags input expects.
function parseTags(tags: string | null): string {
  if (!tags) return '';
  try {
    const parsed = JSON.parse(tags) as unknown;
    if (Array.isArray(parsed)) return parsed.map(String).join(', ');
  } catch {
    // ignore malformed JSON
  }
  return '';
}

/// Mirrors `SyncState` in `src-tauri/src/sync_state.rs`.
type SyncState = 'clean' | 'modified' | 'remote_ahead' | 'conflict' | 'sync_failed';

/// This post's sync state, read from the whole-library query the posts list
/// already uses. A blog's worth of rows is small enough that a second command
/// for one of them would be surface without benefit.
async function readSyncState(
  invoke: <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>,
  id: number,
): Promise<SyncState> {
  try {
    const states = await invoke<{ post_id: number; state: SyncState }[]>('list_sync_states');
    return states.find((s) => s.post_id === id)?.state ?? 'clean';
  } catch {
    return 'clean';
  }
}

/// The editor's content, as compared against what is already stored.
type Content = { title: string; tags: string; body: string };

function sameContent(a: Content, b: Content): boolean {
  return a.title === b.title && a.tags === b.tags && a.body === b.body;
}

// Editor save/publish status, for button feedback.
type SaveState =
  | { kind: 'idle' }
  | { kind: 'saving'; publish: boolean }
  | { kind: 'saved'; publish: boolean }
  | { kind: 'error'; message: string };

// ─── PostEditor ───────────────────────────────────────────────────────────────

export function PostEditor() {
  const [title, setTitle] = useState('');
  const [tags, setTags] = useState('');
  const [body, setBody] = useState('');

  const [postId, setPostId] = useState<number | null>(null);
  const [saveState, setSaveState] = useState<SaveState>({ kind: 'idle' });
  // Whether this post is live, and whether what is live is what is here. A new
  // post is neither, so it starts clean and unpublished.
  const [live, setLive] = useState(false);
  const [sync, setSync] = useState<SyncState>('clean');

  /// The content last known to be on disk, so the editor can tell whether what
  /// is on screen has been written down anywhere yet.
  const persisted = useRef<Content>({ title: '', tags: '', body: '' });

  /// Pull one post's metadata, body and sync state out of the backend into the
  /// editor. Used on mount and again after resolving a conflict, where keeping
  /// the cloud's copy replaces everything on screen.
  const loadFromBackend = useCallback(
    async (
      invoke: <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>,
      id: number,
      keepGoing: () => boolean = () => true,
    ) => {
      const post = await invoke<{
        title: string;
        tags: string | null;
        slug: string;
        published: boolean;
      } | null>('get_post', { id });
      if (!post || !keepGoing()) return;
      setTitle(post.title);
      setTags(parseTags(post.tags));
      setLive(post.published);
      const md = await invoke<string>('read_post_markdown', { slug: post.slug });
      if (!keepGoing()) return;
      setBody(md);
      // What was just loaded is what is on disk, so nothing on screen is
      // unsaved until the author types.
      persisted.current = { title: post.title, tags: parseTags(post.tags), body: md };
      const state = await readSyncState(invoke, id);
      if (keepGoing()) setSync(state);
    },
    [],
  );

  /// Settle a conflict by picking a side. Keeping the cloud's copy overwrites
  /// what is on screen, so the editor reloads rather than leaving the replaced
  /// text sitting in the textarea where the next save would push it back up.
  const resolve = async (keep: 'keep_local' | 'keep_remote') => {
    if (postId === null || saveState.kind === 'saving') return;
    const { invoke, isTauri } = await import('@tauri-apps/api/core');
    if (!isTauri()) return;
    setSaveState({ kind: 'saving', publish: false });
    try {
      await invoke('resolve_conflict', { postId, keep });
      await loadFromBackend(invoke, postId);
      setSaveState({ kind: 'idle' });
    } catch (err) {
      setSaveState({ kind: 'error', message: String(err) });
      setTimeout(() => setSaveState({ kind: 'idle' }), 6000);
    }
  };

  // When the editor is opened with an `?id=` in the URL, load that post's
  // metadata and Markdown body. read_post_markdown downloads the file from R2
  // into the local cache when it isn't already cached locally.
  useEffect(() => {
    const id = Number(new URLSearchParams(window.location.search).get('id'));
    if (!id) return; // no id → new post
    setPostId(id);
    let cancelled = false;
    (async () => {
      const { invoke, isTauri } = await import('@tauri-apps/api/core');
      if (!isTauri()) return;
      try {
        await loadFromBackend(invoke, id, () => !cancelled);
      } catch (err) {
        console.error('Failed to load post:', err);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [loadFromBackend]);

  /// Write what is on screen to disk before a restore replaces it.
  ///
  /// The panel promises that the version being left is kept, and the version
  /// being left is the one the author is looking at. `restore_revision`
  /// snapshots what is *stored*, though, so edits made since the last save
  /// would be captured by nothing and then overwritten by the reload that
  /// follows — the one loss this whole feature exists to prevent, arrived at
  /// through the button labelled Restore.
  ///
  /// Saving them first puts them in the history twice over: this save records
  /// the version before them, and the restore records them.
  ///
  /// A draft save, never a publish. Restoring is a local act, and a rollback
  /// that pushed the author's unsaved paragraph to the blog on the way past
  /// would be a considerably worse surprise than the one being fixed.
  const flushBeforeRestore = async () => {
    if (postId === null) return;
    const content: Content = { title, tags, body };
    if (sameContent(content, persisted.current)) return;
    const { invoke, isTauri } = await import('@tauri-apps/api/core');
    if (!isTauri()) return;
    await invoke('save_post', { id: postId, ...content, published: false });
    persisted.current = content;
  };

  // Save the post: `publish=false` keeps it a local draft; `publish=true` also
  // pushes the body to R2 and metadata to D1 (see the `save_post` command).
  const handleSave = async (publish: boolean) => {
    if (saveState.kind === 'saving') return;
    const { invoke, isTauri } = await import('@tauri-apps/api/core');
    if (!isTauri()) return;
    setSaveState({ kind: 'saving', publish });
    try {
      // Captured before the await, so the baseline recorded below is the text
      // that actually went to disk rather than whatever has been typed since.
      const content: Content = { title, tags, body };
      const saved = await invoke<{ id: number; published: boolean }>('save_post', {
        id: postId,
        ...content,
        published: publish,
      });
      persisted.current = content;
      setPostId(saved.id);
      // Point the URL at the saved post so a refresh / next save targets it.
      window.history.replaceState(null, '', `/posts/edit?id=${saved.id}`);
      // Re-read rather than assume: a publish that reached the cloud clears the
      // pending edits, one that failed does not, and the backend is the only
      // thing that knows which happened.
      setLive(saved.published);
      setSync(await readSyncState(invoke, saved.id));
      setSaveState({ kind: 'saved', publish });
      setTimeout(() => setSaveState({ kind: 'idle' }), 3000);
    } catch (err) {
      setSaveState({ kind: 'error', message: String(err) });
      setTimeout(() => setSaveState({ kind: 'idle' }), 6000);
      // A failed publish is exactly when the badge matters most: the post was
      // saved locally and staged `sync_failed`, and the error message here is
      // on a timer. Without this the pill would not appear until the page was
      // reloaded, and the post would look fine the moment the message cleared.
      // A brand-new post whose first save failed has no id yet, so there is
      // nothing to read.
      if (postId !== null) setSync(await readSyncState(invoke, postId));
    }
  };

  const [mode, setMode] = useState<EditorMode>('write');
  const [preview, setPreview] = useState('');
  const [historyOpen, setHistoryOpen] = useState(false);

  /// Re-read the post after a restore. The backend has replaced the row and the
  /// cached Markdown, so whatever is in the textarea is a version that no longer
  /// exists — leaving it there would let the next save push it straight back
  /// over the thing that was just restored.
  const reloadAfterRestore = () => {
    if (postId === null) return;
    void (async () => {
      const { invoke, isTauri } = await import('@tauri-apps/api/core');
      if (!isTauri()) return;
      try {
        await loadFromBackend(invoke, postId);
      } catch (err) {
        console.error('Failed to reload the restored post:', err);
      }
    })();
  };

  // The preview palette is bound to the app theme in markdown-theme.css (the
  // preset's light-dark() colors don't follow next-themes' class — see that
  // file). We still set `color-scheme` on the container so native UI inside the
  // preview (scrollbars, etc.) matches the resolved theme.
  const { resolvedTheme } = useTheme();
  const previewColorScheme = resolvedTheme === 'dark' ? 'dark' : 'light';

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

  // ── Inline formatting: right-click menu + keyboard shortcuts ────────────────
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  const [pickerOpen, setPickerOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);
  const savedSel = useRef<{ start: number; end: number }>({ start: 0, end: 0 });

  const [isMac, setIsMac] = useState(false);
  useEffect(() => {
    setIsMac(/mac|iphone|ipad/i.test(navigator.userAgent));
  }, []);
  const shortcut = (k: string, shift = false) =>
    isMac ? `⌘${shift ? '⇧' : ''}${k}` : `Ctrl+${shift ? 'Shift+' : ''}${k}`;

  // Wrap the given range in Markdown markers (bold/italic/strike) or raw HTML
  // (underline), toggling the markers back off when they already surround it.
  function surround(before: string, after: string, start: number, end: number) {
    const el = textareaRef.current;
    if (!el) return;
    const value = el.value;
    const hasBefore = value.slice(start - before.length, start) === before;
    const hasAfter = value.slice(end, end + after.length) === after;
    // A lone "*" (italic) sitting next to another "*" is really part of a bold
    // "**" pair — don't mistake that for an italic wrap to toggle off.
    const italicOnBold =
      before === '*' && (value.slice(start - 2, start - 1) === '*' || value.slice(end + 1, end + 2) === '*');
    if (hasBefore && hasAfter && !italicOnBold) {
      const inner = value.slice(start, end);
      applyEdit(
        value.slice(0, start - before.length) + inner + value.slice(end + after.length),
        start - before.length,
        end - before.length,
      );
      return;
    }
    const selected = value.slice(start, end);
    applyEdit(
      value.slice(0, start) + before + selected + after + value.slice(end),
      start + before.length,
      start + before.length + selected.length,
    );
  }

  function insertLink(start: number, end: number) {
    const el = textareaRef.current;
    if (!el) return;
    const value = el.value;
    const prefix = `[${value.slice(start, end)}](`;
    // Drop in a "url" placeholder and select it so the author can type over it.
    applyEdit(
      value.slice(0, start) + prefix + 'url)' + value.slice(end),
      start + prefix.length,
      start + prefix.length + 3,
    );
  }

  // Right-clicking a selection opens the formatting menu; with no selection we
  // leave the native context menu alone.
  function handleEditorContextMenu(e: React.MouseEvent<HTMLTextAreaElement>) {
    const el = e.currentTarget;
    if (el.selectionStart === el.selectionEnd) return;
    e.preventDefault();
    savedSel.current = { start: el.selectionStart, end: el.selectionEnd };
    setMenu({ x: e.clientX, y: e.clientY });
  }

  // Drop a Markdown block in at the caret, sitting it on its own lines and
  // adding blank lines only where the surrounding text doesn't already have
  // them. Shared by drag-and-drop and the media picker.
  function insertBlock(markdown: string) {
    const el = textareaRef.current;
    if (!el) return;
    const value = el.value;
    const at = el.selectionStart;
    const before = value.slice(0, at);
    const after = value.slice(at);
    const lead = before !== '' && !before.endsWith('\n') ? '\n' : '';
    const trail = after !== '' && !after.startsWith('\n') ? '\n' : '';
    const insertion = lead + markdown + trail;
    applyEdit(before + insertion + after, at + insertion.length);
  }

  // "Insert media" → pick from the library. The chosen object is staged into
  // the post's local assets so it publishes under the post's own prefix, the
  // same route a dropped image takes.
  async function pickFromLibrary(entry: MediaEntry) {
    setPickerOpen(false);
    const { invoke, isTauri } = await import('@tauri-apps/api/core');
    if (!isTauri()) return;
    try {
      const staged = await invoke<StagedImage>('stage_media_from_library', { key: entry.key });
      const alt = staged.name.replace(/\.[^.]+$/, '');
      insertBlock(`![${alt}](${staged.rel})`);
    } catch (err) {
      console.error('Failed to insert media from library:', err);
    }
  }

  const formatActions: {
    label: string;
    Icon: LucideIcon;
    keys: string;
    run: () => void;
    separated?: boolean;
  }[] = [
    {
      label: 'Bold',
      Icon: Bold,
      keys: shortcut('B'),
      run: () => surround('**', '**', savedSel.current.start, savedSel.current.end),
    },
    {
      label: 'Italic',
      Icon: Italic,
      keys: shortcut('I'),
      run: () => surround('*', '*', savedSel.current.start, savedSel.current.end),
    },
    {
      label: 'Underline',
      Icon: Underline,
      keys: shortcut('U'),
      run: () => surround('<u>', '</u>', savedSel.current.start, savedSel.current.end),
    },
    {
      label: 'Strikethrough',
      Icon: Strikethrough,
      keys: shortcut('X', true),
      run: () => surround('~~', '~~', savedSel.current.start, savedSel.current.end),
    },
    {
      label: 'Insert Link',
      Icon: Link2,
      keys: shortcut('K'),
      separated: true,
      run: () => insertLink(savedSel.current.start, savedSel.current.end),
    },
  ];

  // Dismiss the menu on outside pointer, Escape, scroll or resize.
  useEffect(() => {
    if (!menu) return;
    const close = () => setMenu(null);
    const onPointerDown = (e: PointerEvent) => {
      if (!menuRef.current?.contains(e.target as Node)) close();
    };
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') close();
    };
    window.addEventListener('pointerdown', onPointerDown, true);
    window.addEventListener('keydown', onKeyDown);
    window.addEventListener('resize', close);
    window.addEventListener('scroll', close, true);
    return () => {
      window.removeEventListener('pointerdown', onPointerDown, true);
      window.removeEventListener('keydown', onKeyDown);
      window.removeEventListener('resize', close);
      window.removeEventListener('scroll', close, true);
    };
  }, [menu]);

  // Once rendered, nudge the menu back inside the viewport if it overflows.
  useEffect(() => {
    if (!menu || !menuRef.current) return;
    const { width, height } = menuRef.current.getBoundingClientRect();
    const pad = 8;
    const x = Math.max(pad, Math.min(menu.x, window.innerWidth - width - pad));
    const y = Math.max(pad, Math.min(menu.y, window.innerHeight - height - pad));
    if (x !== menu.x || y !== menu.y) setMenu({ x, y });
  }, [menu]);

  function handleEditorKeyDown(e: React.KeyboardEvent<HTMLTextAreaElement>) {
    if (e.nativeEvent.isComposing) return; // don't interfere with IME
    const { value, selectionStart: start, selectionEnd: end } = e.currentTarget;

    // Ctrl/Cmd formatting shortcuts (mirror the right-click menu).
    if ((e.ctrlKey || e.metaKey) && !e.altKey) {
      const k = e.key.toLowerCase();
      if (k === 'b') {
        e.preventDefault();
        surround('**', '**', start, end);
        return;
      }
      if (k === 'i') {
        e.preventDefault();
        surround('*', '*', start, end);
        return;
      }
      if (k === 'u') {
        e.preventDefault();
        surround('<u>', '</u>', start, end);
        return;
      }
      if (k === 'k') {
        e.preventDefault();
        insertLink(start, end);
        return;
      }
      if (e.shiftKey && k === 'x') {
        e.preventDefault();
        surround('~~', '~~', start, end);
        return;
      }
    }

    // Tab / Shift+Tab → indent or outdent by one level.
    if (e.key === 'Tab' && !e.ctrlKey && !e.metaKey && !e.altKey) {
      e.preventDefault();
      const outdent = e.shiftKey;

      // A plain caret (no selection) outside a list/quote keeps the simple
      // "insert four spaces at the caret" behaviour on Tab.
      const caretLineStart = value.lastIndexOf('\n', start - 1) + 1;
      let caretLineEnd = value.indexOf('\n', start);
      if (caretLineEnd === -1) caretLineEnd = value.length;
      if (!outdent && start === end && !isListOrQuote(value.slice(caretLineStart, caretLineEnd))) {
        applyEdit(value.slice(0, start) + INDENT + value.slice(end), start + INDENT.length);
        return;
      }

      // Otherwise indent/outdent every line the selection touches by one level.
      const blockStart = caretLineStart;
      // A selection ending exactly at a line start shouldn't pull in the next line.
      const searchFrom = end > start && value[end - 1] === '\n' ? end - 1 : end;
      let blockEnd = value.indexOf('\n', searchFrom);
      if (blockEnd === -1) blockEnd = value.length;

      const lines = value.slice(blockStart, blockEnd).split('\n');
      const newLines = lines.map(outdent ? outdentLine : indentLine);
      const firstDelta = newLines[0].length - lines[0].length;
      const totalDelta = newLines.reduce((sum, l, i) => sum + (l.length - lines[i].length), 0);

      const next = value.slice(0, blockStart) + newLines.join('\n') + value.slice(blockEnd);
      const newStart = Math.max(blockStart, start + firstDelta);
      applyEdit(next, newStart, Math.max(newStart, end + totalDelta));
      return;
    }

    // Enter → continue the current list item / blockquote automatically.
    if (e.key === 'Enter' && !e.shiftKey && !e.ctrlKey && !e.metaKey && !e.altKey && start === end) {
      const lineStart = value.lastIndexOf('\n', start - 1) + 1;
      let lineEnd = value.indexOf('\n', start);
      if (lineEnd === -1) lineEnd = value.length;
      const cont = continuationMarker(value.slice(lineStart, lineEnd));
      if (!cont) return; // ordinary newline
      e.preventDefault();
      if (cont.isEmpty) {
        // Empty item: drop the marker and end the list/quote.
        applyEdit(value.slice(0, lineStart) + value.slice(lineEnd), lineStart);
      } else {
        const insertion = '\n' + cont.marker;
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
    if (mode === 'write') return;
    if (body.trim() === '') {
      setPreview('');
      return;
    }

    let cancelled = false;
    const timer = setTimeout(() => {
      renderMarkdown(body)
        .then(resolveAssetSrcs)
        .then((html) => {
          if (!cancelled) setPreview(html);
        })
        .catch(() => {
          if (!cancelled) setPreview('<p class="md-error">Failed to render preview.</p>');
        });
    }, 200);

    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [mode, body]);

  // Drag-and-drop image insertion (Tauri desktop only). Dropped image files are
  // copied into the app's local assets dir by the `stage_image` command, and a
  // Markdown reference is inserted at the caret; resolveAssetSrcs (above) turns
  // those refs into asset-protocol URLs so they render in the preview.
  const [dragActive, setDragActive] = useState(false);

  useEffect(() => {
    if (!isTauri()) return;
    let unlisten: (() => void) | undefined;
    let disposed = false;
    let draggingImage = false;

    // Tauri reports the pointer in physical pixels; the textarea rect is in CSS
    // pixels, so divide by the device pixel ratio before hit-testing.
    const overEditor = (pos: { x: number; y: number }) => {
      const el = textareaRef.current;
      if (!el) return false;
      const dpr = window.devicePixelRatio || 1;
      const x = pos.x / dpr;
      const y = pos.y / dpr;
      const r = el.getBoundingClientRect();
      return x >= r.left && x <= r.right && y >= r.top && y <= r.bottom;
    };

    const insertImages = async (paths: string[]) => {
      const images = paths.filter((p) => IMAGE_EXT.test(p));
      if (images.length === 0) return;
      const el = textareaRef.current;
      if (!el) return;
      const value = el.value;
      const at = el.selectionStart;

      const refs: string[] = [];
      for (const path of images) {
        try {
          const staged = await invoke<StagedImage>('stage_image', { srcPath: path });
          const alt = staged.name.replace(/\.[^.]+$/, '');
          refs.push(`![${alt}](${staged.rel})`);
        } catch (err) {
          console.error('Failed to stage dropped image:', err);
        }
      }
      if (refs.length === 0) return;

      // Sit the image(s) on their own block, adding blank lines only as needed.
      const before = value.slice(0, at);
      const after = value.slice(at);
      const lead = before !== '' && !before.endsWith('\n') ? '\n' : '';
      const trail = after !== '' && !after.startsWith('\n') ? '\n' : '';
      const insertion = lead + refs.join('\n\n') + trail;
      applyEdit(before + insertion + after, at + insertion.length);
    };

    getCurrentWebview()
      .onDragDropEvent(({ payload }) => {
        if (payload.type === 'enter') {
          draggingImage = payload.paths.some((p) => IMAGE_EXT.test(p));
          setDragActive(draggingImage && overEditor(payload.position));
        } else if (payload.type === 'over') {
          setDragActive(draggingImage && overEditor(payload.position));
        } else if (payload.type === 'leave') {
          draggingImage = false;
          setDragActive(false);
        } else if (payload.type === 'drop') {
          const onEditor = draggingImage && overEditor(payload.position);
          draggingImage = false;
          setDragActive(false);
          if (onEditor) void insertImages(payload.paths);
        }
      })
      .then((fn) => {
        if (disposed) fn();
        else unlisten = fn;
      });

    return () => {
      disposed = true;
      unlisten?.();
    };
    // Registered once; the handler reads the textarea's live value and only
    // stable refs/setters, so it never needs to re-subscribe.
  }, []);

  // ── Shared fields, composed differently per layout mode below ────────────────

  const titleField = (
    <input
      type='text'
      value={title}
      onChange={(e) => setTitle(e.target.value)}
      placeholder='Post title'
      className={[
        'w-full px-4 pt-8 pb-1',
        'text-[26px] font-bold leading-snug tracking-tight',
        'text-zinc-900 dark:text-zinc-50',
        'placeholder:text-zinc-200 dark:placeholder:text-zinc-800',
        'bg-transparent border-none outline-none focus:ring-0',
        'shrink-0',
      ].join(' ')}
    />
  );

  const tagsField = (
    <div className='flex items-center gap-2 px-4 py-2 shrink-0'>
      <Tag size={11} strokeWidth={1.8} className='text-zinc-300 dark:text-zinc-700 shrink-0' />
      <input
        type='text'
        value={tags}
        onChange={(e) => setTags(e.target.value)}
        placeholder='Tags (comma-separated)'
        className={[
          'flex-1 text-[12px] font-medium',
          'text-zinc-500 dark:text-zinc-500',
          'placeholder:text-zinc-300 dark:placeholder:text-zinc-700',
          'bg-transparent border-none outline-none focus:ring-0',
        ].join(' ')}
      />
      <span className='text-zinc-200 dark:text-zinc-800 shrink-0'>·</span>
      <span className='text-[11px] font-mono tracking-tight text-zinc-300 dark:text-zinc-700 shrink-0'>{today()}</span>
    </div>
  );

  const divider = <Separator className='bg-zinc-100 dark:bg-white/[0.04] mx-4 mb-2 w-[calc(100%-2rem)]' />;

  const editor = (
    <textarea
      ref={textareaRef}
      value={body}
      onChange={(e) => setBody(e.target.value)}
      onKeyDown={handleEditorKeyDown}
      onContextMenu={handleEditorContextMenu}
      placeholder={`Start writing in Markdown…\n\n## Heading\n\nYour content here.`}
      spellCheck
      className={cn(
        'flex-1 min-h-0 w-full resize-none',
        'px-4 py-3',
        'font-mono text-[13.5px] leading-[1.85]',
        'text-zinc-700 dark:text-zinc-300',
        'placeholder:text-zinc-300 dark:placeholder:text-zinc-700',
        'bg-transparent border-none outline-none focus:ring-0',
        'overflow-y-auto transition-colors',
        dragActive && 'ring-2 ring-inset ring-blue-400/70 bg-blue-50/50 dark:ring-blue-400/50 dark:bg-blue-500/[0.07]',
      )}
    />
  );

  const previewBody =
    body.trim() === '' ? (
      <p className='font-mono text-[13px] text-zinc-300 dark:text-zinc-700'>Nothing to preview yet.</p>
    ) : preview === '' ? (
      <p className='font-mono text-[13px] text-zinc-300 dark:text-zinc-700'>Rendering…</p>
    ) : (
      // Intentionally unsanitized: `body` is the author's own Markdown, rendered
      // locally in a single-user preview (self-XSS only), and the preset relies
      // on injecting its own HTML (inline copy-button handler, SVG alert icons).
      // SANITIZE INSTEAD AT THE PUBLISH/READ PATH — where post HTML is served to
      // other readers or synced from an untrusted source — ideally server-side
      // in the Rust backend. See memory: project_markdown_sanitize_at_publish.
      <div
        className='md-body text-[14px]'
        style={{ colorScheme: previewColorScheme }}
        dangerouslySetInnerHTML={{ __html: preview }}
      />
    );

  return (
    <div className='flex flex-col flex-1 min-h-0 bg-white dark:bg-[#161616]'>
      <MediaPicker open={pickerOpen} onClose={() => setPickerOpen(false)} onPick={pickFromLibrary} />
      <RevisionHistory
        open={historyOpen}
        postId={postId}
        onClose={() => setHistoryOpen(false)}
        onBeforeRestore={flushBeforeRestore}
        onRestored={reloadAfterRestore}
      />

      {/* ── Topbar ──────────────────────────────────────────────────────── */}
      <div className='relative flex items-center justify-between px-5 h-[48px] shrink-0 border-b border-zinc-200 dark:border-white/[0.06]'>
        {/* Back */}
        <Button
          asChild
          variant='ghost'
          size='sm'
          className='h-[28px] px-2 -ml-1 gap-1.5 rounded-[5px] text-[12px] font-medium text-zinc-500 dark:text-zinc-400'
        >
          <Link href='/posts'>
            <ArrowLeft size={13} strokeWidth={2} />
            Back to Posts
          </Link>
        </Button>

        {/* Write / Split / Preview toggle */}
        <div className='absolute left-1/2 -translate-x-1/2 flex items-center gap-0.5 p-[3px] rounded-[7px] bg-zinc-100 dark:bg-white/[0.04]'>
          {(
            [
              { value: 'write', label: 'Write', Icon: PenLine },
              { value: 'split', label: 'Split', Icon: Columns2 },
              { value: 'preview', label: 'Preview', Icon: Eye },
            ] as const
          ).map(({ value, label, Icon }) => (
            <button
              key={value}
              type='button'
              onClick={() => setMode(value)}
              aria-pressed={mode === value}
              className={cn(
                'flex items-center gap-1.5 h-[22px] px-2.5 rounded-[5px] text-[12px] font-medium transition-colors active:scale-95',
                mode === value
                  ? 'bg-white dark:bg-white/[0.08] text-zinc-900 dark:text-zinc-100 shadow-[0_1px_2px_rgba(0,0,0,0.08)]'
                  : 'text-zinc-500 dark:text-zinc-400 hover:text-zinc-800 dark:hover:text-zinc-200',
              )}
            >
              <Icon size={12} strokeWidth={2} />
              {label}
            </button>
          ))}
        </div>

        {/* Actions */}
        <div className='flex items-center gap-2'>
          {/* What readers are being served, when that differs from what is on
              screen. Silent when the two agree — a badge that is always there
              stops being read. */}
          {saveState.kind === 'idle' && sync === 'conflict' && (
            <>
              <StatusPill status='conflict' />
              {/* Both answers throw something away, so neither is a default and
                  neither is styled as the safe one. */}
              <Button
                variant='outline'
                size='sm'
                onClick={() => void resolve('keep_local')}
                title='Keep what is on this machine. The cloud change is discarded; your edits stay unpublished until you publish them.'
                className='h-[28px] px-2 rounded-[5px] text-[12px] font-semibold'
              >
                Keep mine
              </Button>
              <Button
                variant='outline'
                size='sm'
                onClick={() => void resolve('keep_remote')}
                title='Take the cloud version. Everything on screen is replaced by it.'
                className='h-[28px] px-2 rounded-[5px] text-[12px] font-semibold'
              >
                Keep cloud
              </Button>
            </>
          )}
          {saveState.kind === 'idle' && sync === 'remote_ahead' && <StatusPill status='behind' />}
          {saveState.kind === 'idle' && sync === 'sync_failed' && <StatusPill status='failed' />}
          {saveState.kind === 'idle' && sync === 'modified' && live && <StatusPill status='edited' />}
          {saveState.kind === 'saved' && (
            <span className='text-[12px] font-medium text-emerald-600 dark:text-emerald-400'>
              {saveState.publish ? 'Published' : 'Saved'}
            </span>
          )}
          {saveState.kind === 'error' && (
            <span
              title={saveState.message}
              className='max-w-[220px] truncate text-[12px] font-medium text-red-600 dark:text-red-400'
            >
              {saveState.message}
            </span>
          )}

          <Button
            variant='ghost'
            size='sm'
            onClick={() => setPickerOpen(true)}
            title='Insert an image from the media library'
            className='h-[28px] px-2 gap-1.5 rounded-[5px] text-[12px] font-medium text-zinc-500 dark:text-zinc-400'
          >
            <ImagePlus size={13} strokeWidth={2} />
            Insert media
          </Button>

          {/* Hidden until the post has been saved once: history is kept from
              the first save onwards, so a post that has never been saved has
              nothing to offer behind this button. */}
          {postId !== null && (
            <Button
              variant='ghost'
              size='sm'
              onClick={() => setHistoryOpen(true)}
              title='Earlier versions of this post, and a way back to any of them'
              className='h-[28px] px-2 gap-1.5 rounded-[5px] text-[12px] font-medium text-zinc-500 dark:text-zinc-400'
            >
              <History size={13} strokeWidth={2} />
              History
            </Button>
          )}

          <Button
            variant='outline'
            size='sm'
            onClick={() => handleSave(false)}
            disabled={saveState.kind === 'saving'}
            className='h-[28px] px-3 rounded-[5px] text-[12px] font-semibold text-zinc-600 dark:text-zinc-400'
          >
            {saveState.kind === 'saving' && !saveState.publish ? 'Saving…' : 'Save Draft'}
          </Button>

          <Button
            size='sm'
            onClick={() => handleSave(true)}
            disabled={saveState.kind === 'saving'}
            className='h-[28px] px-3 rounded-[5px] text-[12px] font-semibold shadow-[0_1px_2px_rgba(0,0,0,0.12)] hover:shadow-[0_2px_8px_rgba(0,0,0,0.18)] dark:hover:shadow-[0_2px_8px_rgba(0,0,0,0.5)]'
          >
            {saveState.kind === 'saving' && saveState.publish ? 'Publishing…' : 'Publish'}
          </Button>
        </div>
      </div>

      {/* ── Document area ───────────────────────────────────────────────── */}
      {mode === 'split' ? (
        /* VS Code-style split: Markdown source (left) + live preview (right). */
        <div className='flex-1 min-h-0 flex flex-col w-full'>
          <div className='shrink-0 w-full'>
            {titleField}
            {tagsField}
          </div>
          {divider}
          <div className='flex-1 min-h-0 flex'>
            <div className='flex-1 min-w-0 min-h-0 flex flex-col overflow-hidden'>{editor}</div>
            <div className='flex-1 min-w-0 min-h-0 overflow-y-auto px-5 py-3 border-l border-zinc-200 dark:border-white/[0.06]'>
              {previewBody}
            </div>
          </div>
        </div>
      ) : (
        <div className='flex-1 min-h-0 flex flex-col max-w-[760px] w-full mx-auto px-2'>
          {titleField}
          {tagsField}
          {divider}
          {mode === 'write' ? (
            editor
          ) : (
            <div className='flex-1 min-h-0 w-full overflow-y-auto px-4 py-3'>{previewBody}</div>
          )}
        </div>
      )}

      {/* ── Status bar ──────────────────────────────────────────────────── */}
      <div className='flex items-center justify-between px-5 h-[34px] shrink-0 border-t border-zinc-100 dark:border-white/[0.04]'>
        <div className='flex items-center gap-3'>
          <span className='text-[11px] font-mono text-zinc-300 dark:text-zinc-700'>
            {words.toLocaleString()} {words === 1 ? 'word' : 'words'}
          </span>
          <span className='text-zinc-200 dark:text-zinc-800'>·</span>
          <span className='text-[11px] font-mono text-zinc-300 dark:text-zinc-700'>
            {chars.toLocaleString()} {chars === 1 ? 'character' : 'characters'}
          </span>
        </div>
        <span className='text-[10px] font-bold uppercase tracking-[0.1em] text-zinc-300 dark:text-zinc-700'>
          Markdown
        </span>
      </div>

      {/* ── Selection formatting menu (right-click) ─────────────────────── */}
      {menu &&
        createPortal(
          <div
            ref={menuRef}
            role='menu'
            style={{ top: menu.y, left: menu.x }}
            className='fixed z-50 min-w-[188px] rounded-lg border border-zinc-200 bg-white p-1 shadow-xl shadow-black/[0.06] dark:border-white/10 dark:bg-zinc-900 dark:shadow-black/40'
          >
            {formatActions.map(({ label, Icon, keys, run, separated }) => (
              <Fragment key={label}>
                {separated && <div className='my-1 h-px bg-zinc-100 dark:bg-white/10' />}
                <button
                  type='button'
                  role='menuitem'
                  onClick={() => {
                    run();
                    setMenu(null);
                  }}
                  className='flex w-full items-center gap-2.5 rounded-md px-2 py-1.5 text-[13px] font-medium text-zinc-700 transition-colors hover:bg-zinc-100 active:scale-[0.98] dark:text-zinc-200 dark:hover:bg-white/[0.06]'
                >
                  <Icon size={14} strokeWidth={2} className='text-zinc-500 dark:text-zinc-400' />
                  {label}
                  <span className='ml-auto pl-4 font-mono text-[11px] text-zinc-400 dark:text-zinc-600'>{keys}</span>
                </button>
              </Fragment>
            ))}
          </div>,
          document.body,
        )}
    </div>
  );
}
