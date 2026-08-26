'use client';

import { Fragment, useCallback, useEffect, useRef, useState } from 'react';
import { createPortal, flushSync } from 'react-dom';
import {
  ArrowLeft,
  Bold,
  CalendarClock,
  Columns2,
  Eye,
  History,
  ImagePlus,
  Italic,
  Layers,
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
import { mediaMarkup } from '@/lib/media';
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

// Editor save/publish status, for button feedback.
type SaveState =
  | { kind: 'idle' }
  | { kind: 'saving'; publish: boolean }
  | { kind: 'saved'; publish: boolean }
  | { kind: 'error'; message: string };

/// Where the editor's text stands relative to this machine's copy of it —
/// nothing to do with the cloud, which autosave never touches.
type LocalSaveState = { kind: 'idle' } | { kind: 'saving' } | { kind: 'saved' } | { kind: 'failed'; message: string };

/// How long the editor waits after the last keystroke before writing to disk.
///
/// Short enough that closing the app after a thought is finished keeps it, long
/// enough that ordinary typing is one write rather than one per word.
const AUTOSAVE_DELAY_MS = 1500;

/// Mirrors `PublishWarning` in `src-tauri/src/commands/r2.rs`.
type PublishWarning = { kind: 'dead_asset'; reference: string } | { kind: 'no_excerpt' };

/// Mirrors `ScheduleView` in `src-tauri/src/commands/r2.rs`.
type Schedule = {
  slug: string;
  publish_at: number;
  state: 'scheduled' | 'overdue' | 'published' | 'failed' | 'cancelled' | 'unknown';
  error: string | null;
  updated_at: number;
};

/// This post's pending publication, if it has one. Read from the whole-library
/// query for the same reason the sync state is: a blog's worth of schedules is a
/// handful of rows, and a second command for one of them would be surface
/// without benefit.
async function readSchedule(
  invoke: <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>,
  slug: string,
): Promise<Schedule | null> {
  try {
    const rows = await invoke<Schedule[]>('list_schedules');
    return rows.find((s) => s.slug === slug) ?? null;
  } catch {
    return null;
  }
}

/// A `datetime-local` value for the browser's own idea of local time.
///
/// `toISOString` would be an hour or thirteen wrong depending on where the
/// author is: it converts to UTC, and this input is read as wall-clock time.
function localInputValue(date: Date): string {
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

/// The editor's content, as compared against what is already stored.
type Content = { title: string; tags: string; body: string };

function sameContent(a: Content, b: Content): boolean {
  return a.title === b.title && a.tags === b.tags && a.body === b.body;
}

// ─── PostEditor ───────────────────────────────────────────────────────────────

/// The series a post can be filed under. Mirrors the columns of `list_series`
/// that this screen needs.
type SeriesOption = { id: number; title: string };

export function PostEditor() {
  const [title, setTitle] = useState('');
  const [tags, setTags] = useState('');
  const [body, setBody] = useState('');

  const [postId, setPostId] = useState<number | null>(null);

  /// The series this post is filed under, and where it sits in one.
  ///
  /// Not part of the save. `content_hash` covers what a reader would notice
  /// of the post's own content, and membership of a series is not in it — so
  /// filing a post is written through `set_post_series` the moment it is
  /// chosen, rather than waiting for a Save that would report the post as
  /// edited when none of its text changed.
  const [seriesList, setSeriesList] = useState<SeriesOption[]>([]);
  const [seriesId, setSeriesId] = useState<number | null>(null);
  const [seriesOrder, setSeriesOrder] = useState('');
  const [seriesError, setSeriesError] = useState<string | null>(null);
  /// The post's thumbnail — the card image the blog derives from the slug.
  ///
  /// Like series filing, and for the same reason: it is not part of
  /// `content_hash`, so it is written the moment it is chosen rather than
  /// waiting for a Save that would report the post as edited when none of its
  /// text changed. Unlike series, it lives only in R2 — nothing about it is on
  /// the post's row — so what is held here is a staged copy to look at, not the
  /// value itself.
  const [thumbnail, setThumbnail] = useState<string | null>(null);
  const [thumbnailBusy, setThumbnailBusy] = useState(false);
  const [thumbnailError, setThumbnailError] = useState<string | null>(null);
  /// What a pre-publish check found, held while the author decides whether to go
  /// ahead.
  ///
  /// Non-null means a publish was asked for and is waiting on an answer, not
  /// that anything is wrong with the post — every one of these is a warning the
  /// author is free to overrule.
  const [publishWarnings, setPublishWarnings] = useState<PublishWarning[] | null>(null);
  /// The filing as it stands, for the save that gives a new post its id —
  /// there is nothing to write it to until then.
  const seriesRef = useRef<{ id: number | null; order: number | null }>({ id: null, order: null });
  const [saveState, setSaveState] = useState<SaveState>({ kind: 'idle' });
  const [localSave, setLocalSave] = useState<LocalSaveState>({ kind: 'idle' });

  // ── What autosave needs to know, outside of React's render cycle ────────────
  //
  // The flush can be triggered by a timer or by the editor closing, neither of
  // which re-renders first, so the values it writes are read from refs rather
  // than captured in a closure that may be a keystroke out of date.

  /// The content last known to be on disk. Autosave compares against this, so
  /// typing a character and deleting it again writes nothing.
  const persisted = useRef<Content>({ title: '', tags: '', body: '' });
  /// The content as it is right now.
  const latest = useRef<Content>({ title: '', tags: '', body: '' });
  /// The post id as it is right now — a new post gains one mid-session, and the
  /// unmount flush has to write to it rather than create a second post.
  const postIdRef = useRef<number | null>(null);
  /// Whether a post is being read out of the backend right now.
  ///
  /// Autosave must stand well clear of that. `loadFromBackend` sets the title
  /// and tags as soon as the row arrives and the body only once it has been
  /// fetched — which reaches R2 for a post this machine has not cached — so for
  /// as long as that download takes, the editor is showing a real title over an
  /// empty body. A flush in that window writes the empty body over the post,
  /// and then the load finishes and records the downloaded text as persisted:
  /// the editor believes the right thing is on disk, nothing corrects it, and
  /// every later read gets the emptied cache.
  const loadingRef = useRef(false);
  /// The same fact as `loadingRef`, kept as state so the Save and Publish
  /// buttons can show that they are unavailable. The ref stays the authority
  /// for the guards — it is written synchronously, and a render behind is
  /// exactly the window this is guarding.
  const [loadingBody, setLoadingBody] = useState(false);
  /// The pending debounce, so a manual save can cancel it rather than have it
  /// fire again straight afterwards.
  const autosaveTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  /// The timer that returns the save banner to idle.
  ///
  /// Held so each operation can cancel the last one's. These are armed per
  /// operation and fire blind, so a finished save's timer would otherwise clear
  /// the banner belonging to the publish that followed it — and `handleSave`
  /// reads that banner to decide whether a save is already running, so clearing
  /// it mid-upload reopens the door to a second `save_post` with `published:
  /// true`. The History button, disabled for the same reason, comes back too.
  const bannerTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  /// Every write to the save banner goes through here, so no write can leave an
  /// earlier operation's reset still armed. `resetAfterMs` is for the terminal
  /// states that clear themselves; the rest cancel and stay put.
  const showSaveState = useCallback((next: SaveState, resetAfterMs?: number) => {
    if (bannerTimer.current !== null) {
      clearTimeout(bannerTimer.current);
      bannerTimer.current = null;
    }
    setSaveState(next);
    if (resetAfterMs !== undefined) {
      bannerTimer.current = setTimeout(() => {
        bannerTimer.current = null;
        setSaveState({ kind: 'idle' });
      }, resetAfterMs);
    }
  }, []);
  /// Tail of the chain of writes to this post.
  ///
  /// Autosave and the Save/Publish buttons write the same row and the same
  /// file, and a timer can fire while a click is already in flight. Left to
  /// race, the *older* text can land second: an autosave that started before
  /// Publish would overwrite the local body afterwards, leaving the machine a
  /// version behind the blog and the post reporting edits that are older than
  /// what readers are served. Queueing them makes last-issued mean last-written.
  const writes = useRef<Promise<unknown>>(Promise.resolve());
  // Whether this post is live, and whether what is live is what is here. A new
  // post is neither, so it starts clean and unpublished.
  const [live, setLive] = useState(false);
  const [sync, setSync] = useState<SyncState>('clean');
  /// Bumped whenever a load finishes, purely to re-run the debounce effect —
  /// see the `finally` in `loadFromBackend`.
  const [loadEpoch, setLoadEpoch] = useState(0);
  /// The post's slug, which is what a schedule is keyed by. Only known once the
  /// post has been saved at least once.
  const [slug, setSlug] = useState<string | null>(null);
  const [schedule, setSchedule] = useState<Schedule | null>(null);
  /// Whether the "publish later" controls are open, and what has been picked.
  const [scheduling, setScheduling] = useState(false);
  const [scheduleAt, setScheduleAt] = useState('');

  /// Pull one post's metadata, body and sync state out of the backend into the
  /// editor. Used on mount and again after resolving a conflict, where keeping
  /// the cloud's copy replaces everything on screen.
  const loadFromBackend = useCallback(
    async (
      invoke: <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>,
      id: number,
      keepGoing: () => boolean = () => true,
    ) => {
      // Held for the whole load, not just the fetch: the title lands on screen
      // before the body does, and an autosave in between would write the empty
      // body over the post. See `loadingRef`.
      loadingRef.current = true;
      setLoadingBody(true);
      // The gate lifts only once a *complete* load has established the
      // baseline. A read that fails partway — an uncached body whose R2 fetch
      // cannot reach the network — leaves the editor showing a real title over
      // an empty body with no baseline behind it, which is precisely the state
      // an autosave must not be allowed to write. Autosave stays off for the
      // rest of the session in that case, and says so; a stalled timer is
      // recoverable, an emptied cache masking the remote Markdown is not.
      let loaded = false;
      try {
        const post = await invoke<{
          title: string;
          tags: string | null;
          slug: string;
          published: boolean;
          series_id: number | null;
          series_order: number | null;
        } | null>('get_post', { id });
        if (!post || !keepGoing()) return;
        setTitle(post.title);
        setTags(parseTags(post.tags));
        setLive(post.published);
        setSeriesId(post.series_id);
        setSeriesOrder(post.series_order === null ? '' : String(post.series_order));
        seriesRef.current = { id: post.series_id, order: post.series_order };
        const md = await invoke<string>('read_post_markdown', { slug: post.slug });
        if (!keepGoing()) return;
        setBody(md);
        // What was just loaded *is* what is on disk, so autosave has nothing to
        // do until something changes. Without this, opening a post would write
        // it straight back — and, for a published post, report unpublished
        // edits nobody made.
        persisted.current = { title: post.title, tags: parseTags(post.tags), body: md };
        setSlug(post.slug);
        loaded = true;
        const state = await readSyncState(invoke, id);
        if (keepGoing()) setSync(state);
        const schedule = await readSchedule(invoke, post.slug);
        if (keepGoing()) setSchedule(schedule);
      } catch (err) {
        setLocalSave({
          kind: 'failed',
          message: `Autosave is off — this post did not finish loading: ${String(err)}`,
        });
        throw err;
      } finally {
        if (loaded) {
          loadingRef.current = false;
          setLoadingBody(false);
          // The debounce effect reads a ref, so it needs telling that the
          // answer has changed — otherwise an edit made *during* the load would
          // sit unscheduled until the next keystroke.
          setLoadEpoch((n) => n + 1);
        }
      }
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
    // Take the write from autosave, exactly as a manual save does. Keeping the
    // cloud's copy installs a downloaded body; an autosave landing after it
    // would put the editor's stale text straight back over that body and mark
    // the post modified — silently undoing the choice the person just made.
    if (autosaveTimer.current !== null) {
      clearTimeout(autosaveTimer.current);
      autosaveTimer.current = null;
    }
    showSaveState({ kind: 'saving', publish: false });
    try {
      // "Keep mine" settles the conflict on what is *stored*, so what is on
      // screen has to be stored first — cancelling the debounce above would
      // otherwise drop the last second and a half of typing, and the reload
      // afterwards would replace the editor with the older copy that won.
      //
      // "Keep cloud" discards local work by definition, so there is nothing to
      // flush and the cancelled timer is exactly right.
      if (keep === 'keep_local') await flushPending();
      // Through the queue, so an autosave already in flight finishes first and
      // this lands after it rather than under it.
      await enqueueWrite(() => invoke('resolve_conflict', { postId, keep }));
      await loadFromBackend(invoke, postId);
      showSaveState({ kind: 'idle' });
    } catch (err) {
      showSaveState({ kind: 'error', message: String(err) }, 6000);
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

  // ── Autosave ────────────────────────────────────────────────────────────────

  /// Run `write` once everything queued before it has finished, and hand back
  /// its result.
  ///
  /// One writer at a time, in the order the writes were asked for — see
  /// [`writes`]. A failure does not poison the queue: the next write is still
  /// allowed to run, because the usual cause is the cloud being unreachable and
  /// the usual next write is a local one that has no opinion about that.
  const enqueueWrite = useCallback(<T,>(write: () => Promise<T>): Promise<T> => {
    const run = writes.current.then(write, write);
    writes.current = run.catch(() => {});
    return run;
  }, []);

  /// Write the editor's current text to this machine, and to nowhere else.
  ///
  /// `autosave_post` has no publish flag at all, so no timer can ever push a
  /// half-written paragraph to the blog — the promise that autosave is local is
  /// a property of the command, not of this function remembering to pass
  /// `false`.
  ///
  /// Stable across renders: the timer and the unmount flush both hold a
  /// reference to it, and re-creating it on every keystroke would leave those
  /// pointing at an older copy.
  const persistLocally = useCallback(
    () =>
      enqueueWrite(async () => {
        // Read after the queue has drained, not before: a manual save ahead of
        // this one has already stored its text and moved `persisted`, and the
        // usual answer here is that there is nothing left to write.
        // A load may have started while this sat in the queue, and a load is
        // exactly when the editor's state is half a post. See `loadingRef`.
        if (loadingRef.current) return;

        const content = latest.current;
        if (sameContent(content, persisted.current)) return;

        // Autosave writes to posts; it does not create them. A post's slug is
        // derived from its title when the row is first written and never moves
        // again — nothing in the app can edit it — so a timer firing partway
        // through a title would fix the URL as `/my` for "My Complete Post",
        // permanently and silently. The first Save or Publish is the moment the
        // author has decided what the post is called, and that is the moment
        // worth deriving a permanent name from.
        //
        // The cost is that a post which has never been saved at all is not
        // protected by autosave. That is the smaller loss: an unsaved new draft
        // is one Ctrl-S from safety and is visibly unsaved, whereas a wrong slug
        // is invisible, permanent, and public once published.
        const id = postIdRef.current;
        if (id === null) return;

        const { invoke, isTauri } = await import('@tauri-apps/api/core');
        if (!isTauri()) return;

        setLocalSave({ kind: 'saving' });
        try {
          const saved = await invoke<{ id: number; published: boolean }>('autosave_post', {
            id,
            ...content,
            series: pendingSeries(),
          });
          // Recorded before anything else can fail: this text is on disk now,
          // and the next flush must not write it again.
          persisted.current = content;
          // Only if nothing has moved since. Typing while a write is in flight
          // leaves this content already out of date on arrival, and announcing
          // it as saved would put "Saved locally" over text that is still
          // waiting for the next debounce.
          setLocalSave(sameContent(latest.current, content) ? { kind: 'saved' } : { kind: 'idle' });
          setSync(await readSyncState(invoke, saved.id));
        } catch (err) {
          // Deliberately sticky, and deliberately harmless: the editor's
          // contents are untouched, so the text is still there to be saved by
          // hand. A message that cleared itself would be the one thing worse
          // than no message, since the next keystroke schedules another attempt
          // anyway.
          setLocalSave({ kind: 'failed', message: String(err) });
        }
      }),
    [enqueueWrite],
  );

  /// Write what is on screen to disk, and refuse to carry on if it did not
  /// land.
  ///
  /// Used by the two operations that go on to *replace* the editor's contents
  /// from what is stored — restoring a revision, and settling a conflict by
  /// keeping the local copy. Both would otherwise act on the last flushed
  /// version and then reload it over the text on screen, discarding anything
  /// typed in the second and a half since. Autosave makes that a narrow window
  /// and not a closed one, which is not the promise either operation makes.
  ///
  /// `persistLocally` reports its own failures rather than throwing, so the
  /// check is on the outcome: if what is on screen is still not what is on
  /// disk, the caller must not proceed over it.
  const flushPending = async () => {
    if (postIdRef.current === null) return;
    await persistLocally();
    if (!sameContent(latest.current, persisted.current)) {
      throw new Error('Could not save the current version first');
    }
  };

  // The values the flush reads. Kept in refs because it can run from a timer or
  // from the editor closing, neither of which renders first.
  useEffect(() => {
    latest.current = { title, tags, body };
  }, [title, tags, body]);

  useEffect(() => {
    postIdRef.current = postId;
  }, [postId]);

  // Debounce: every change restarts the clock, and the write happens once the
  // typing stops. An edit that leaves the content as it was found schedules
  // nothing at all.
  useEffect(() => {
    // Nothing is scheduled while a post is being read in. The title arrives
    // before the body, and a timer started on that half-loaded state would be
    // counting down towards writing an empty body over the post — `loadEpoch`
    // re-runs this once the load is done, so an edit made during it is not
    // forgotten either.
    if (loadingRef.current) return;
    if (sameContent({ title, tags, body }, persisted.current)) return;
    // "Saved locally" was about the previous content, and this content is not
    // on disk yet — least of all during continuous typing, where the message
    // would otherwise stand unchanged for as long as somebody keeps writing,
    // right up to a close whose flush is explicitly best effort.
    //
    // A failure is left where it is: it is sticky by design, and the flush that
    // eventually succeeds is what clears it.
    setLocalSave((current) => (current.kind === 'saved' ? { kind: 'idle' } : current));
    const timer = setTimeout(() => void persistLocally(), AUTOSAVE_DELAY_MS);
    autosaveTimer.current = timer;
    return () => {
      clearTimeout(timer);
      if (autosaveTimer.current === timer) autosaveTimer.current = null;
    };
  }, [title, tags, body, loadEpoch, persistLocally]);

  // Leaving the editor with a debounce still pending would lose whatever was
  // typed in the last second and a half. Both exits are covered: navigating
  // away unmounts, and closing the window fires `beforeunload`.
  //
  // Best effort on the second one — the write is asynchronous and the window
  // may go first — which is why the debounce is short rather than clever.
  useEffect(() => {
    const flush = () => void persistLocally();
    window.addEventListener('beforeunload', flush);
    return () => {
      window.removeEventListener('beforeunload', flush);
      flush();
    };
  }, [persistLocally]);

  // A banner reset still armed when the editor closes would set state on a
  // component that is gone.
  useEffect(
    () => () => {
      if (bannerTimer.current !== null) clearTimeout(bannerTimer.current);
    },
    [],
  );

  // ── Scheduling ──────────────────────────────────────────────────────────────

  /// Hand the post to Cloudflare to publish at the chosen time.
  ///
  /// Everything is uploaded now — see the `schedule_post` command — so the post
  /// goes live whether or not this app is running when its time comes. Any
  /// pending local edits are saved first, because what is scheduled is what is
  /// uploaded, and uploading a version the author has since changed would
  /// publish something nobody chose.
  ///
  /// Through `flushPending`, so a local save that did not land stops this
  /// outright. Autosave reports its failures and carries on, which is right for
  /// a timer and wrong here: the difference is a version going live at an hour
  /// when nobody is watching, and the error message would already be gone.
  const submitSchedule = async () => {
    if (postId === null || scheduleAt === '') return;
    const at = Math.floor(new Date(scheduleAt).getTime() / 1000);
    if (Number.isNaN(at)) return;

    const { invoke, isTauri } = await import('@tauri-apps/api/core');
    if (!isTauri()) return;
    showSaveState({ kind: 'saving', publish: false });
    try {
      await flushPending();
      await enqueueWrite(() => invoke('schedule_post', { postId, publishAt: at }));
      if (slug !== null) setSchedule(await readSchedule(invoke, slug));
      setScheduling(false);
      showSaveState({ kind: 'idle' });
      setSync(await readSyncState(invoke, postId));
    } catch (err) {
      showSaveState({ kind: 'error', message: String(err) }, 6000);
    }
  };

  /// Call off a pending publication. The post stays exactly where it is — an
  /// unpublished draft whose body happens to already be in R2, invisible to
  /// readers until something publishes it.
  const cancelSchedule = async () => {
    if (postId === null) return;
    const { invoke, isTauri } = await import('@tauri-apps/api/core');
    if (!isTauri()) return;
    showSaveState({ kind: 'saving', publish: false });
    try {
      await enqueueWrite(() => invoke('cancel_schedule', { postId }));
      if (slug !== null) setSchedule(await readSchedule(invoke, slug));
      showSaveState({ kind: 'idle' });
    } catch (err) {
      showSaveState({ kind: 'error', message: String(err) }, 6000);
    }
  };

  // Save the post: `publish=false` keeps it a local draft; `publish=true` also
  // pushes the body to R2 and metadata to D1 (see the `save_post` command).
  const handleSave = async (publish: boolean, checked = false) => {
    if (saveState.kind === 'saving') return;
    // Nothing may be written while the body is still on its way in: until the
    // load completes the editor holds a real title over an empty one, and saving
    // from there writes that emptiness over the post — into R2 too, on a publish.
    // See `loadingRef`, which fences autosave off from the same window. The ref
    // rather than the state beside it, because one render's lag is the whole of
    // what is being guarded.
    if (loadingRef.current) return;
    const { invoke, isTauri } = await import('@tauri-apps/api/core');
    if (!isTauri()) return;

    // Look the post over before it goes live, once. `checked` is the answer
    // coming back from the panel below: the author has read what was found and
    // said go ahead, and asking again from there would be an argument rather
    // than a check.
    //
    // Only on publish, and only for a post that already exists — a first save
    // has no id to check against, and it is the publish that cannot be taken
    // back locally, not the save.
    if (publish && !checked && postIdRef.current !== null) {
      try {
        const found = await invoke<PublishWarning[]>('check_post_before_publish', {
          id: postIdRef.current,
          // The editor's text, not what is on disk: this save is about to
          // publish what is on screen.
          body,
        });
        if (found.length > 0) {
          setPublishWarnings(found);
          return;
        }
      } catch {
        // A check that cannot run is not a reason to stand between somebody and
        // publishing their post. Nothing here is load-bearing.
      }
    }
    setPublishWarnings(null);

    // Take the write from autosave: a pending debounce firing straight after
    // this would be a second write of text this one has already stored.
    if (autosaveTimer.current !== null) {
      clearTimeout(autosaveTimer.current);
      autosaveTimer.current = null;
    }
    setLocalSave({ kind: 'idle' });
    showSaveState({ kind: 'saving', publish });
    try {
      // What is being saved, captured before the await so the baseline recorded
      // below is the text that actually went to disk rather than whatever has
      // been typed since.
      const content: Content = { title, tags, body };
      // Behind any autosave already in flight, and — through the ref rather
      // than the state — aimed at the post autosave may have just created.
      // Reading the state here would send `id: null` for a post that exists,
      // and make a second one.
      const saved = await enqueueWrite(() =>
        // The slug comes back with the row and is taken from it. A post created
        // by this very save has none on screen yet, and scheduling is keyed by
        // slug — so without this, scheduling a just-saved post succeeded while
        // the button went on offering to schedule it, with no way to cancel
        // until the editor was closed and reopened.
        invoke<{ id: number; slug: string; published: boolean }>('save_post', {
          id: postIdRef.current,
          ...content,
          published: publish,
          // On the row this save writes, not applied after it returns: a first
          // save that publishes has already sent the post to D1 by then, and it
          // would go live outside the series it was filed into.
          series: pendingSeries(),
        }),
      );
      persisted.current = content;
      setPostId(saved.id);
      postIdRef.current = saved.id;
      setSlug(saved.slug);
      // Point the URL at the saved post so a refresh / next save targets it.
      window.history.replaceState(null, '', `/posts/edit?id=${saved.id}`);
      // Re-read rather than assume: a publish that reached the cloud clears the
      // pending edits, one that failed does not, and the backend is the only
      // thing that knows which happened.
      setLive(saved.published);
      setSync(await readSyncState(invoke, saved.id));
      showSaveState({ kind: 'saved', publish }, 3000);
    } catch (err) {
      showSaveState({ kind: 'error', message: String(err) }, 6000);
      // A failed publish is exactly when the badge matters most: the post was
      // saved locally and staged `sync_failed`, and the error message here is
      // on a timer. Without this the pill would not appear until the page was
      // reloaded, and the post would look fine the moment the message cleared.
      // The ref, not the state: this handler closed over `postId` before the
      // save was queued, and a save that ran ahead of it may have given the
      // post an id since. Reading the stale `null` would skip the refresh and
      // leave the pre-publish state on screen once the error message clears,
      // in exactly the case the badge matters most.
      //
      // A brand-new post whose first save failed still has no id anywhere, so
      // there is genuinely nothing to read.
      const current = postIdRef.current;
      if (current !== null) setSync(await readSyncState(invoke, current));
      // `persisted` deliberately stays where it was. A failed publish may well
      // have stored the text locally before the upload failed — `save_post`
      // does the local half first — but the error does not say so, and an
      // autosave that repeats a write that already landed costs nothing, while
      // skipping one that did not would lose the edit.
    }
  };

  const [mode, setMode] = useState<EditorMode>('write');
  const [preview, setPreview] = useState('');
  const [historyOpen, setHistoryOpen] = useState(false);

  /// Re-read the post after a restore. The backend has replaced the row and the
  /// cached Markdown, so whatever is in the textarea is a version that no longer
  /// exists — leaving it there would let the next save push it straight back
  /// over the thing that was just restored.
  const reloadAfterRestore = async () => {
    if (postId === null) return;
    const { invoke, isTauri } = await import('@tauri-apps/api/core');
    if (!isTauri()) return;
    // Failures propagate to the history panel, which keeps itself open and says
    // so. Swallowing them here would close the panel over an editor still
    // showing the version that was just replaced.
    await loadFromBackend(invoke, postId);
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
      // An image tag around a video renders as a broken image and nothing else,
      // which is what picking a video from the library used to produce.
      insertBlock(mediaMarkup(staged.rel, staged.name));
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

      // Read after the staging above, not before it. Copying a phone-sized photo
      // takes long enough to type a sentence into, and the textarea is not
      // disabled while it happens — so a snapshot taken beforehand is a document
      // that has since moved on, and rebuilding from it threw away every
      // character typed in between and moved the caret to match. `insertBlock`,
      // which the media picker uses, already reads its value after awaiting.
      //
      // The insertion point is the caret as it stands now, for the same reason:
      // the one captured earlier belongs to a document this one no longer is.
      const value = el.value;
      const at = el.selectionStart;

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

  // The series available to file into. Local read, and small — there are only
  // ever a handful — so it is fetched once and not refreshed while the editor
  // is open. A series made after this point appears the next time it opens.
  useEffect(() => {
    if (!isTauri()) return;
    let live = true;
    void invoke<SeriesOption[]>('list_series')
      .then((rows) => {
        if (live) setSeriesList(rows);
      })
      .catch(() => {
        // A failure here costs the dropdown its options, which is visible on
        // its own. The editor still saves, publishes, and schedules.
      });
    return () => {
      live = false;
    };
  }, []);

  /// The series to hand a save that may be creating the post. Ignored by the
  /// backend when the post already exists, which is why it can be sent every
  /// time rather than only on the first save.
  const pendingSeries = () =>
    seriesRef.current.id === null ? null : { id: seriesRef.current.id, order: seriesRef.current.order };

  /// File the post, or take it out of a series.
  ///
  /// A post that does not exist yet has nothing to write to, so the choice is
  /// held in `seriesRef` and applied by the save that creates it.
  const applySeries = async (nextId: number | null, nextOrder: number | null) => {
    setSeriesId(nextId);
    setSeriesOrder(nextOrder === null ? '' : String(nextOrder));
    seriesRef.current = { id: nextId, order: nextOrder };
    setSeriesError(null);

    const id = postIdRef.current;
    if (id === null || !isTauri()) return;
    try {
      await enqueueWrite(() => invoke('set_post_series', { postId: id, seriesId: nextId, seriesOrder: nextOrder }));
    } catch (err) {
      // Said out loud rather than swallowed: the control would otherwise show a
      // filing the database does not have.
      setSeriesError(String(err));
    }
  };

  /// Fetch the thumbnail and hand the webview something it is allowed to show.
  ///
  /// Quiet on failure. This runs whenever a post is opened, and a library with
  /// no credentials configured — or a post that simply has no thumbnail — is not
  /// a thing to put an error in front of somebody about. Setting one says so
  /// out loud; looking for one does not.
  const loadThumbnail = useCallback(async (forSlug: string) => {
    if (!isTauri()) return;
    try {
      const rel = await invoke<string | null>('stage_post_thumbnail', { slug: forSlug });
      setThumbnail(rel === null ? null : convertFileSrc(await join(await appDataDir(), rel)));
    } catch {
      setThumbnail(null);
    }
  }, []);

  useEffect(() => {
    // A post that has never been saved has no slug, and the thumbnail's key is
    // derived from the slug alone — there is nowhere for one to be yet.
    if (slug === null) {
      setThumbnail(null);
      return;
    }
    void loadThumbnail(slug);
  }, [slug, loadThumbnail]);

  const applyThumbnail = async () => {
    if (slug === null || thumbnailBusy || !isTauri()) return;
    setThumbnailBusy(true);
    setThumbnailError(null);
    try {
      await enqueueWrite(() => invoke('set_post_thumbnail', { slug }));
      await loadThumbnail(slug);
    } catch (err) {
      const msg = String(err);
      // Dismissing the file dialog is not a failure — the same distinction the
      // posts list draws around `export_post`.
      if (msg !== 'cancelled') setThumbnailError(msg);
    } finally {
      setThumbnailBusy(false);
    }
  };

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

  const seriesField = (
    <div className='flex items-center gap-2 px-4 pb-2 shrink-0'>
      <Layers size={11} strokeWidth={1.8} className='text-zinc-300 dark:text-zinc-700 shrink-0' />
      <select
        aria-label='Series'
        value={seriesId ?? ''}
        onChange={(e) => {
          const next = e.target.value === '' ? null : Number(e.target.value);
          // Dropping out of a series drops the position with it: an order with
          // no series to be ordered within is a number about nothing.
          void applySeries(next, next === null ? null : seriesRef.current.order);
        }}
        className={[
          'text-[12px] font-medium max-w-[240px]',
          seriesId === null ? 'text-zinc-300 dark:text-zinc-700' : 'text-zinc-500 dark:text-zinc-500',
          'bg-transparent border-none outline-none focus:ring-0',
          'transition-colors cursor-pointer',
        ].join(' ')}
      >
        <option value=''>No series</option>
        {seriesList.map((s) => (
          <option key={s.id} value={s.id}>
            {s.title}
          </option>
        ))}
      </select>

      {seriesId !== null && (
        <>
          <span className='text-zinc-200 dark:text-zinc-800 shrink-0'>·</span>
          <input
            type='number'
            min={1}
            aria-label='Position in series'
            value={seriesOrder}
            onChange={(e) => setSeriesOrder(e.target.value)}
            // Written on blur rather than per keystroke: every digit typed
            // would otherwise be a separate write, and `12` would pass through
            // being `1`.
            onBlur={() => {
              const parsed = seriesOrder.trim() === '' ? null : Number(seriesOrder);
              const next = parsed !== null && Number.isFinite(parsed) ? Math.trunc(parsed) : null;
              if (next !== seriesRef.current.order) void applySeries(seriesId, next);
            }}
            placeholder='#'
            className={[
              'w-[42px] text-[12px] font-medium',
              'text-zinc-500 dark:text-zinc-500',
              'placeholder:text-zinc-300 dark:placeholder:text-zinc-700',
              'bg-transparent border-none outline-none focus:ring-0',
            ].join(' ')}
          />
        </>
      )}

      {seriesError && <span className='truncate text-[11px] text-red-600 dark:text-red-400'>{seriesError}</span>}
    </div>
  );

  const thumbnailField = (
    <div className='flex items-center gap-2 px-4 pb-2 shrink-0'>
      <ImagePlus size={11} strokeWidth={1.8} className='text-zinc-300 dark:text-zinc-700 shrink-0' />
      {thumbnail !== null && (
        // `asset:` URL from the local staging directory, not a remote image
        // next/image could optimise. The media library and picker carry the
        // same exemption for the same reason.
        // eslint-disable-next-line @next/next/no-img-element
        <img
          src={thumbnail}
          alt='Current thumbnail'
          className='h-[20px] w-[36px] shrink-0 rounded-[2px] border border-zinc-200 object-cover dark:border-white/[0.08]'
        />
      )}
      <button
        type='button'
        onClick={() => void applyThumbnail()}
        disabled={slug === null || thumbnailBusy}
        // Said rather than left to be guessed: a disabled control with no
        // reason on it reads as broken.
        title={slug === null ? 'Save the post first — a thumbnail is stored under its slug' : undefined}
        className={[
          'text-[12px] font-medium transition-colors active:scale-95',
          slug === null || thumbnailBusy
            ? 'cursor-not-allowed text-zinc-300 dark:text-zinc-700'
            : 'cursor-pointer text-zinc-500 hover:text-zinc-800 dark:text-zinc-500 dark:hover:text-zinc-200',
        ].join(' ')}
      >
        {thumbnailBusy ? 'Uploading…' : thumbnail !== null ? 'Replace thumbnail' : 'Set thumbnail'}
      </button>

      {thumbnailError && <span className='truncate text-[11px] text-red-600 dark:text-red-400'>{thumbnailError}</span>}
    </div>
  );

  /// What the check found, and the way past it.
  ///
  /// Deliberately not a modal. Nothing here is an error, and a dialog demanding
  /// to be dismissed would make a note read like a refusal — the author can go
  /// on editing with this on screen, fix what they meant to fix, and press
  /// Publish again.
  const publishWarningsPanel = publishWarnings !== null && (
    <div className='mx-4 mb-2 shrink-0 rounded-[6px] border border-amber-200 bg-amber-50/60 px-3 py-2 dark:border-amber-900/40 dark:bg-amber-950/20'>
      <p className='text-[12px] font-medium text-amber-800 dark:text-amber-400'>Worth a look before this goes live</p>
      <ul className='mt-1 space-y-[2px]'>
        {publishWarnings.map((w, i) => (
          <li key={i} className='text-[12px] leading-[1.6] text-amber-700 dark:text-amber-500'>
            {w.kind === 'no_excerpt' ? (
              <>No excerpt — it is the card text and the meta description.</>
            ) : (
              <>
                <span className='font-mono'>{w.reference}</span> is not on this machine. It will publish as a dead link.
              </>
            )}
          </li>
        ))}
      </ul>
      <div className='mt-2 flex items-center gap-2'>
        <Button
          size='sm'
          variant='ghost'
          onClick={() => setPublishWarnings(null)}
          className='h-[24px] px-2 rounded-[4px] text-[12px]'
        >
          Go back
        </Button>
        <Button
          size='sm'
          onClick={() => void handleSave(true, true)}
          className='h-[24px] px-2 rounded-[4px] text-[12px] font-semibold'
        >
          Publish anyway
        </Button>
      </div>
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
        onBeforeRestore={flushPending}
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
          {/* Autosave, which is about this machine only — never the blog. Kept
              out of the way of the manual save's own feedback, and silent until
              autosave has actually done something. */}
          {saveState.kind === 'idle' && localSave.kind !== 'idle' && (
            <span
              title={
                localSave.kind === 'failed'
                  ? localSave.message
                  : 'Autosave keeps this post on this machine only. Publishing is still a separate, deliberate step.'
              }
              className={cn(
                'max-w-[220px] truncate text-[12px] font-medium',
                localSave.kind === 'failed' ? 'text-red-600 dark:text-red-400' : 'text-zinc-400 dark:text-zinc-500',
              )}
            >
              {localSave.kind === 'saving'
                ? 'Saving locally…'
                : localSave.kind === 'saved'
                  ? 'Saved locally'
                  : 'Autosave failed'}
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
              nothing to offer behind this button.

              Disabled while a save or publish is in flight, for the same reason
              those buttons disable themselves — and one more. A restore during a
              slow publish would run its own save and its own rollback alongside
              a command that finishes by recording the hash of the version it
              started with, leaving the restored text marked as the version the
              cloud holds. */}
          {postId !== null && (
            <Button
              variant='ghost'
              size='sm'
              onClick={() => setHistoryOpen(true)}
              disabled={saveState.kind === 'saving'}
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
            disabled={saveState.kind === 'saving' || loadingBody}
            title={loadingBody ? 'Waiting for this post to finish loading' : undefined}
            className='h-[28px] px-3 rounded-[5px] text-[12px] font-semibold text-zinc-600 dark:text-zinc-400'
          >
            {saveState.kind === 'saving' && !saveState.publish ? 'Saving…' : 'Save Draft'}
          </Button>

          {/* Publishing later. Offered only for a saved, unpublished post: a
              live post has nothing to schedule, and a post with no row yet has
              no slug for the schedule to name. */}
          {postId !== null && !live && (
            <div className='relative'>
              {schedule?.state === 'scheduled' || schedule?.state === 'overdue' ? (
                <Button
                  variant='outline'
                  size='sm'
                  onClick={() => void cancelSchedule()}
                  disabled={saveState.kind === 'saving'}
                  title={`Cloudflare publishes this at ${new Date(schedule.publish_at * 1000).toLocaleString()}. Cancelling leaves it an unpublished draft.`}
                  className='h-[28px] px-2 gap-1.5 rounded-[5px] text-[12px] font-semibold text-indigo-600 dark:text-indigo-400'
                >
                  <CalendarClock size={13} strokeWidth={2} />
                  {schedule.state === 'overdue' ? 'Overdue — cancel' : 'Cancel schedule'}
                </Button>
              ) : (
                <Button
                  variant='outline'
                  size='sm'
                  onClick={() => {
                    // Default to an hour from now: far enough ahead to be a real
                    // choice, near enough to be edited rather than retyped.
                    const inAnHour = new Date(Date.now() + 60 * 60 * 1000);
                    setScheduleAt(localInputValue(inAnHour));
                    setScheduling((open) => !open);
                  }}
                  disabled={saveState.kind === 'saving'}
                  className='h-[28px] px-2 gap-1.5 rounded-[5px] text-[12px] font-semibold text-zinc-600 dark:text-zinc-400'
                >
                  <CalendarClock size={13} strokeWidth={2} />
                  Schedule
                </Button>
              )}

              {scheduling && (
                <div className='absolute right-0 top-[34px] z-50 w-[300px] rounded-[8px] border border-zinc-200 bg-white p-3 shadow-xl shadow-black/[0.06] dark:border-white/10 dark:bg-zinc-900 dark:shadow-black/40'>
                  <label
                    htmlFor='schedule-at'
                    className='block text-[12px] font-semibold text-zinc-700 dark:text-zinc-300'
                  >
                    Publish at
                  </label>
                  <input
                    id='schedule-at'
                    type='datetime-local'
                    value={scheduleAt}
                    onChange={(e) => setScheduleAt(e.target.value)}
                    className='mt-1.5 w-full rounded-[5px] border border-zinc-200 bg-white px-2 py-1.5 text-[12px] text-zinc-800 outline-none focus:border-zinc-400 dark:border-white/10 dark:bg-zinc-950 dark:text-zinc-200 dark:focus:border-white/30'
                  />
                  <p className='mt-2 text-[11px] leading-[1.5] text-zinc-500 dark:text-zinc-500'>
                    The post is uploaded now and goes live at this time, published by Cloudflare — so it happens whether
                    or not this app is running. Readers see nothing until then.
                  </p>
                  <div className='mt-3 flex items-center justify-end gap-2'>
                    <Button
                      variant='ghost'
                      size='sm'
                      onClick={() => setScheduling(false)}
                      className='h-[26px] px-2 rounded-[5px] text-[12px] font-semibold text-zinc-500'
                    >
                      Cancel
                    </Button>
                    <Button
                      size='sm'
                      onClick={() => void submitSchedule()}
                      disabled={scheduleAt === '' || saveState.kind === 'saving'}
                      className='h-[26px] px-3 rounded-[5px] text-[12px] font-semibold'
                    >
                      Schedule
                    </Button>
                  </div>
                </div>
              )}
            </div>
          )}

          <Button
            size='sm'
            onClick={() => handleSave(true)}
            disabled={saveState.kind === 'saving' || loadingBody}
            title={loadingBody ? 'Waiting for this post to finish loading' : undefined}
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
            {seriesField}
            {thumbnailField}
            {publishWarningsPanel}
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
          {seriesField}
          {thumbnailField}
          {publishWarningsPanel}
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
