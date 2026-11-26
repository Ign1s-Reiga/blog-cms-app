'use client';

import { useCallback, useEffect, useRef, useState } from 'react';
import { CheckCircle2, Download, EyeOff, Import, Loader2, Plus, RotateCcw, Search, Trash2 } from 'lucide-react';
import Link from 'next/link';
import { useRouter, useSearchParams } from 'next/navigation';
import { StatusDot, type PostStatus } from '@/components/StatusDot';
import { StatusPill } from '@/components/StatusPill';
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { Input } from '@/components/ui/input';
import { Button } from '@/components/ui/button';
import { Checkbox } from '@/components/ui/checkbox';
import { Badge } from '@/components/ui/badge';
import { Alert, AlertDescription } from '@/components/ui/alert';
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog';
import { BulkActions, type BulkAction, type BulkOutcome } from '@/components/BulkActions';
import { onPostsRefreshed } from '@/lib/sync';
import { cn } from '@/lib/utils';

// ─── Types ────────────────────────────────────────────────────────────────────

type FilterId = 'all' | 'published' | 'edited' | 'conflict' | 'draft' | 'scheduled' | 'failed' | 'trash';

type ImportStatus =
  | { kind: 'idle' }
  | { kind: 'loading' }
  | { kind: 'success'; title: string }
  | { kind: 'error'; message: string };

/// Mirrors `Unchecked` in `src-tauri/src/media_usage.rs`, of which body search
/// can produce two.
type UnsearchedReason = 'body_not_cached' | 'body_stale';

/// Mirrors `Unsearched` in `src-tauri/src/body_search.rs`.
type Unsearched = { id: number; title: string; reason: UnsearchedReason };

/// Mirrors `BodyMatches`, with the query it answered kept alongside it.
///
/// The query is not decoration. Results outliving the text that produced them
/// is how a search for `rust` goes on showing its posts while `tauri` is being
/// typed — the title and tag filters move to the new text immediately, and the
/// body ids from the old one would still be ORed in. Every use is gated on this
/// matching what is in the box now.
///
/// `null` means no body search has answered for the current query yet — which is
/// not the same as one that found nothing.
type BodyMatches = { query: string; matched: number[]; unsearched: Unsearched[] } | null;

/// The shape the backend actually returns; the query is added on arrival.
type BodyMatchesPayload = { matched: number[]; unsearched: Unsearched[] };

/// Said plainly, because each has a different way out.
const UNSEARCHED_REASON: Record<UnsearchedReason, string> = {
  body_not_cached: 'its text is not on this machine',
  body_stale: 'the copy here is older than the published one',
};

type ExportStatus =
  | { kind: 'idle' }
  | { kind: 'working'; id: number }
  /// `unpublishedEdits` means the file carries text readers have not been
  /// served — worth saying, because the file and the blog then disagree.
  | { kind: 'success'; slug: string; path: string; unpublishedEdits: boolean }
  | { kind: 'error'; message: string };

// Row shape the table renders.
type Post = {
  id: number;
  /// Carried because a schedule is keyed by slug — ids do not survive the
  /// crossing into D1, where the Worker reads them.
  slug: string;
  title: string;
  tags: string[];
  status: 'published' | 'draft';
  /// A pending publication, if this post has one.
  schedule?: Schedule;
  /// How the local copy compares with what readers are served. Kept separate
  /// from `status` on purpose: publication and synchronisation are two facts,
  /// and a published post can be carrying edits nobody has seen.
  sync: SyncState;
  date: string;
  views?: number;
};

/// Mirrors `SyncState` in `src-tauri/src/sync_state.rs`.
type SyncState = 'clean' | 'modified' | 'remote_ahead' | 'conflict' | 'sync_failed';

type BackendSyncState = { post_id: number; state: SyncState };

// Subset of the `list_posts` command payload we actually use.
type BackendPost = {
  id: number;
  slug: string;
  title: string;
  tags: string | null; // JSON-encoded string[]
  published: boolean;
  created_at: number; // Unix seconds
};

/// Mirrors `ScheduleView` in `src-tauri/src/commands/r2.rs`. The state is
/// derived in Rust — including `overdue`, which nothing stores — so the desktop
/// and anything else reading these rows agree on what one means.
type Schedule = {
  slug: string;
  publish_at: number;
  state: 'scheduled' | 'overdue' | 'published' | 'failed' | 'cancelled' | 'unknown';
  error: string | null;
  updated_at: number;
};

/// A post in the trash: the same row, plus when it was thrown away and whether
/// it is still on the blog — which trashing deliberately does not change.
type TrashedPost = Post & { trashedAt: string; live: boolean };

/// What the trash view is about to do, held while the person confirms it.
/// Permanent deletion is the one thing in this app that cannot be walked back,
/// so it does not happen on a single click.
type PendingDelete = { kind: 'one'; id: number; title: string } | { kind: 'all'; count: number };

/// When a scheduled publication happens, in the reader's own timezone.
///
/// The author picked a wall-clock time in the editor and the backend stores it
/// as an instant, so showing it back in UTC would name an hour nobody chose —
/// and it sat next to a tooltip that was already local, so the row disagreed
/// with itself.
function formatScheduleTime(unixSeconds: number): string {
  const at = new Date(unixSeconds * 1000);
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${at.getFullYear()}-${pad(at.getMonth() + 1)}-${pad(at.getDate())} ${pad(at.getHours())}:${pad(at.getMinutes())}`;
}

function toPost(p: BackendPost): Post {
  let tags: string[] = [];
  if (p.tags) {
    try {
      const parsed = JSON.parse(p.tags) as unknown;
      if (Array.isArray(parsed)) tags = parsed.map(String);
    } catch {
      // leave tags empty on malformed JSON
    }
  }
  return {
    id: p.id,
    slug: p.slug,
    title: p.title,
    tags,
    status: p.published ? 'published' : 'draft',
    sync: 'clean',
    date: new Date(p.created_at * 1000).toISOString().slice(0, 10),
  };
}

/// The one badge a row shows, from the two facts behind it. A failed push wins:
/// it is the state that needs action. Otherwise a published post carrying local
/// edits reads as `edited` rather than plainly `published`, which is the whole
/// point — the post is live, and this version of it is not.
function displayStatus(post: Post): PostStatus {
  if (post.sync === 'sync_failed') return 'failed';
  // A conflict outranks the rest: it is the only state that cannot be resolved
  // by pressing the button the post would otherwise be offering.
  if (post.sync === 'conflict') return 'conflict';
  if (post.sync === 'remote_ahead') return 'behind';
  if (post.status === 'published' && post.sync === 'modified') return 'edited';
  // A publication that was due and has not happened needs somebody to look at
  // Cloudflare, so it outranks the ordinary "waiting" reading — and a schedule
  // that failed is reported with the same urgency as a failed push.
  if (post.schedule?.state === 'overdue') return 'overdue';
  if (post.schedule?.state === 'failed') return 'failed';
  // Between "draft" and "scheduled", the second says everything the first does
  // and adds when it stops being true.
  if (post.status === 'draft' && post.schedule?.state === 'scheduled') return 'scheduled';
  return post.status;
}

export default function PostsPage() {
  const router = useRouter();
  const [filter, setFilter] = useState<FilterId>('all');
  const [search, setSearch] = useState('');
  /// What the last body search answered for the query as it then stood.
  const [bodyMatches, setBodyMatches] = useState<BodyMatches>(null);
  const [bodySearching, setBodySearching] = useState(false);
  const [fillingGaps, setFillingGaps] = useState(false);
  /// Which body search is the current one. A slow answer for `rust` must not
  /// land on top of a quick one for `rustup` and show the wrong rows.
  const bodyAttempt = useRef(0);
  /// What the search box says *now*, for the code that has to ask after an
  /// await. `search` inside an async function is the value from the render that
  /// created it, so a gap-fill started before the query changed would compare
  /// the old text against itself, conclude nothing had moved, and go on to
  /// retire the newer query's search and answer under it. A ref is read at the
  /// moment of asking rather than at the moment of closing over.
  const liveSearch = useRef(search);
  useEffect(() => {
    liveSearch.current = search;
  }, [search]);
  /// The tag the list is narrowed to, arriving from the Tags screen as
  /// `?tag=`. Held in state rather than read per render so clearing it does not
  /// need a navigation.
  const params = useSearchParams();
  const [tagFilter, setTagFilter] = useState<string | null>(null);
  /// Ids ticked in the list. Held as a Set so the row checkboxes stay cheap,
  /// and cleared whenever the listing is reloaded — an id that is no longer on
  /// screen must not stay selected and be acted on from behind a filter.
  const [selected, setSelected] = useState<Set<number>>(new Set());
  const [importStatus, setImportStatus] = useState<ImportStatus>({ kind: 'idle' });
  /// What the last export produced, or why it did not. Kept apart from
  /// `importStatus` so one does not clear the other's message.
  const [exportStatus, setExportStatus] = useState<ExportStatus>({ kind: 'idle' });

  const [posts, setPosts] = useState<Post[]>([]);
  const [trashed, setTrashed] = useState<TrashedPost[]>([]);
  const [pendingDelete, setPendingDelete] = useState<PendingDelete | null>(null);
  /// The deletion the confirmation is worded for, held past `pendingDelete`
  /// going back to null. The dialog animates out after it closes, and wording
  /// it from `pendingDelete` would blank it for the length of that animation.
  const [shownDelete, setShownDelete] = useState<PendingDelete | null>(pendingDelete);
  if (pendingDelete !== null && pendingDelete !== shownDelete) setShownDelete(pendingDelete);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  /// A trash, unpublish or schedule action that was refused or failed.
  ///
  /// Kept apart from `loadError`, which is only ever rendered in the table's
  /// empty state — a refusal shown there would be invisible on any list with a
  /// post in it, which is every list this can happen on.
  const [actionError, setActionError] = useState<string | null>(null);

  // Load posts from the local SQLite cache via the Tauri backend. No-ops in a
  // plain browser (`pnpm dev`), where the Tauri API isn't available.
  const loadPosts = useCallback(async () => {
    const { invoke, isTauri } = await import('@tauri-apps/api/core');
    if (!isTauri()) {
      setLoading(false);
      return;
    }
    try {
      // Local cache — refreshed from the cloud on launch and via the refresh button.
      const rows = await invoke<BackendPost[]>('list_posts');
      // How each post compares with the cloud (best-effort — doesn't block the
      // list; without it every post simply reads as clean, which is what the
      // list showed before this existed).
      let sync = new Map<number, SyncState>();
      try {
        const states = await invoke<BackendSyncState[]>('list_sync_states');
        sync = new Map(states.map((s) => [s.post_id, s.state]));
      } catch {
        // ignore sync-state query errors
      }
      // Pending publications, keyed by slug — best effort, like the sync states:
      // without them every post simply reads as it did before scheduling
      // existed, which is a good deal better than an empty list.
      let schedules = new Map<string, Schedule>();
      try {
        const pending = await invoke<Schedule[]>('list_schedules');
        schedules = new Map(pending.map((s) => [s.slug, s]));
      } catch {
        // ignore schedule query errors
      }
      // A selection belongs to the listing it was made in: an id that is no
      // longer on screen must not stay ticked and be acted on from behind a
      // filter or a refresh.
      setSelected(new Set());
      setPosts(
        rows.map((p) => ({
          ...toPost(p),
          sync: sync.get(p.id) ?? 'clean',
          schedule: schedules.get(p.slug),
        })),
      );
      // The trash is a separate listing, not a filter over the first one: a
      // trashed post is excluded from `list_posts` by the backend, which is what
      // keeps it out of every other screen too.
      const binned = await invoke<(BackendPost & { trashed_at: number })[]>('list_trashed_posts');
      setTrashed(
        binned.map((p) => ({
          ...toPost(p),
          trashedAt: new Date(p.trashed_at * 1000).toISOString().slice(0, 10),
          live: p.published,
        })),
      );
      setLoadError(null);
    } catch (err) {
      setLoadError(String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadPosts();
  }, [loadPosts]);

  // Re-read local data after a cloud refresh.
  useEffect(() => onPostsRefreshed(() => void loadPosts()), [loadPosts]);

  const handleImportArticle = async () => {
    setImportStatus({ kind: 'loading' });
    try {
      const { invoke } = await import('@tauri-apps/api/core');
      const title = await invoke<string>('import_article');
      setImportStatus({ kind: 'success', title });
      void loadPosts(); // show the newly imported draft
      setTimeout(() => setImportStatus({ kind: 'idle' }), 4000);
    } catch (err) {
      const msg = String(err);
      if (msg === 'cancelled') {
        setImportStatus({ kind: 'idle' });
        return;
      }
      setImportStatus({ kind: 'error', message: msg });
      setTimeout(() => setImportStatus({ kind: 'idle' }), 6000);
    }
  };

  // Bodies are searched on a pause in typing, not on every keystroke: the
  // command walks every cached body, which is right once and wrong ten times a
  // second. Titles and tags keep matching instantly below, so the box stays
  // responsive while this catches up.
  useEffect(() => {
    const q = search.trim();
    if (q === '') {
      setBodyMatches(null);
      setBodySearching(false);
      return;
    }
    setBodySearching(true);
    const mine = ++bodyAttempt.current;
    const timer = setTimeout(async () => {
      try {
        const { invoke, isTauri } = await import('@tauri-apps/api/core');
        if (!isTauri()) return;
        const found = await invoke<BodyMatchesPayload>('search_post_bodies', { query: q });
        if (mine !== bodyAttempt.current) return;
        setBodyMatches({ query: q, ...found });
      } catch {
        // A failed body search leaves title and tag matching working. Clearing
        // to `null` says "no answer" rather than "nothing matched".
        if (mine === bodyAttempt.current) setBodyMatches(null);
      } finally {
        if (mine === bodyAttempt.current) setBodySearching(false);
      }
    }, 300);
    return () => clearTimeout(timer);
  }, [search]);

  /// Fetch the bodies that could not be searched, then search again. The only
  /// part of this feature that touches the network, and only when asked.
  const fillSearchGaps = async () => {
    if (!bodyMatches || bodyMatches.unsearched.length === 0 || fillingGaps) return;
    // The query this was started for. Fetching is slow enough to type through,
    // and answering the old text under the new one is worse than not answering.
    const asked = bodyMatches.query;
    const ids = bodyMatches.unsearched.map((u) => u.id);
    setFillingGaps(true);
    // The attempt this claims, once it claims one. Retiring a debounced search
    // that is still in flight makes its own `finally` a no-op — that guard only
    // clears the spinner for the attempt still current — so whoever does the
    // retiring inherits the spinner and has to put it down again.
    let mine: number | null = null;
    try {
      const { invoke, isTauri } = await import('@tauri-apps/api/core');
      if (!isTauri()) return;
      await invoke('cache_bodies', { ids });
      // Only if the box still says what it said. Claiming a fresh attempt
      // regardless would also retire a debounced search for the *newer* text
      // and leave the old results sitting under it.
      //
      // Read through the ref, not `search`: this function closed over the value
      // as it stood when the button was pressed, so comparing against that
      // would be comparing `asked` with itself and would pass however much the
      // box had moved on.
      if (asked !== liveSearch.current.trim()) return;
      mine = ++bodyAttempt.current;
      setBodySearching(true);
      const found = await invoke<BodyMatchesPayload>('search_post_bodies', { query: asked });
      if (mine === bodyAttempt.current && asked === liveSearch.current.trim()) {
        setBodyMatches({ query: asked, ...found });
      }
    } catch {
      // Whatever could not be fetched stays listed as unsearched, which is
      // already the honest state.
    } finally {
      setFillingGaps(false);
      if (mine !== null && mine === bodyAttempt.current) setBodySearching(false);
    }
  };

  /// Apply one action to every selected post.
  ///
  /// Each post goes through the *same command* the single-post button uses, one
  /// at a time: a bulk publish is the publish, run repeatedly, with the same
  /// staging, revisions and sync bookkeeping. Nothing is rolled back — a post
  /// that succeeded stays done — and a failure is recorded against the post it
  /// belongs to rather than aborting the rest.
  const runBulk = async (action: BulkAction, tag: string): Promise<BulkOutcome> => {
    const { invoke, isTauri } = await import('@tauri-apps/api/core');
    const ids = actionable;
    const outcome: BulkOutcome = { done: 0, failed: [], skipped: [] };
    if (!isTauri()) return outcome;

    const rowOf = (id: number) => posts.find((p) => p.id === id) ?? trashed.find((p) => p.id === id);
    const titleOf = (id: number) => rowOf(id)?.title ?? `Post ${id}`;

    // Tagging is one command over the whole selection, not one per post: it has
    // to re-read each row, snapshot it and refresh its fingerprint in a single
    // transaction, and that lives on the Rust side for exactly that reason.
    if (action === 'addTag' || action === 'removeTag') {
      try {
        const command = action === 'addTag' ? 'add_tag_to_posts' : 'remove_tag_from_posts';
        const result = await invoke<{ changed: number; skipped: BulkOutcome['skipped'] }>(command, {
          ids,
          tag,
        });
        outcome.done = result.changed;
        outcome.skipped = result.skipped;
      } catch (err) {
        // A failure here is the whole call, not one post's.
        outcome.failed.push({ id: 0, title: `${ids.length} posts`, message: String(err) });
      }
      setSelected(new Set());
      await loadPosts();
      return outcome;
    }

    for (const id of ids) {
      try {
        const row = rowOf(id);

        // Both of these send this machine's whole row to D1, so neither may run
        // while the cloud's copy is ahead or in disagreement: they would settle
        // it in the stale local copy's favour on the way past. The per-row
        // Unpublish button hides itself for exactly this reason; a bulk run must
        // not be the way around it.
        if (
          (action === 'publish' || action === 'unpublish') &&
          (row?.sync === 'conflict' || row?.sync === 'remote_ahead')
        ) {
          outcome.failed.push({
            id,
            title: titleOf(id),
            message:
              row.sync === 'conflict'
                ? 'the cloud copy disagrees with this one — resolve it in the editor first'
                : 'the cloud copy is newer — pull or resolve it in the editor first',
          });
          continue;
        }

        switch (action) {
          case 'trash':
            await invoke('trash_post', { id });
            break;
          case 'restore':
            await invoke('restore_post', { id });
            break;
          case 'publish':
            // `publish_stored_post`, not `save_post` with this row's fields.
            // These rows were read when the list last loaded, and `save` writes
            // every column — sending them back would revert a title, excerpt or
            // tag set that moved in between, and publish the reverted version.
            // The command takes an id and re-reads the row and body itself.
            //
            // Not `publish_post` either: that flips the flag and pushes
            // metadata without ever uploading the Markdown, which would put a
            // post live in D1 with no body in R2 and report success anyway.
            await invoke('publish_stored_post', { id });
            break;
          case 'unpublish':
            await invoke('unpublish_post', { postId: id });
            break;
        }
        outcome.done += 1;
      } catch (err) {
        outcome.failed.push({ id, title: titleOf(id), message: String(err) });
      }
    }

    setSelected(new Set());
    await loadPosts();
    return outcome;
  };

  const toggleSelected = (id: number) => {
    setSelected((current) => {
      const next = new Set(current);
      if (!next.delete(id)) next.add(id);
      return next;
    });
  };

  const exportPost = async (id: number) => {
    if (exportStatus.kind === 'working') return;
    const { invoke, isTauri } = await import('@tauri-apps/api/core');
    // Asked before the working state is claimed, not inside it. There is
    // nothing to export to in a browser, and returning from under
    // `{kind:'working'}` used to leave it there for good — the guard above then
    // refused every later export, so `pnpm run dev` lost the button for the rest
    // of the session. A `finally` would clear it, but it would also clear the
    // success and error states below, which are meant to stay on screen.
    if (!isTauri()) return;
    setExportStatus({ kind: 'working', id });
    try {
      const done = await invoke<{ path: string; slug: string; unpublished_edits: boolean }>('export_post', { id });
      setExportStatus({
        kind: 'success',
        slug: done.slug,
        path: done.path,
        unpublishedEdits: done.unpublished_edits,
      });
      setTimeout(() => setExportStatus({ kind: 'idle' }), 6000);
    } catch (err) {
      const msg = String(err);
      // Dismissing the save dialog is not a failure.
      if (msg === 'cancelled') {
        setExportStatus({ kind: 'idle' });
        return;
      }
      setExportStatus({ kind: 'error', message: msg });
      setTimeout(() => setExportStatus({ kind: 'idle' }), 8000);
    }
  };

  /// Run a command that moves a post between listings, and re-read both. They
  /// move together — a restore takes a post out of one and puts it in the other
  /// — so re-reading one of them would leave the tab counts disagreeing with the
  /// rows. Unpublishing does the same to the Published and Drafts counts.
  const runPostCommand = async (command: string, args?: Record<string, unknown>) => {
    const { invoke, isTauri } = await import('@tauri-apps/api/core');
    if (!isTauri()) return;
    setActionError(null);
    try {
      await invoke(command, args);
      await loadPosts();
    } catch (err) {
      setActionError(String(err));
    }
  };

  const confirmDelete = async () => {
    if (!pendingDelete) return;
    const pending = pendingDelete;
    setPendingDelete(null);
    await (pending.kind === 'one'
      ? runPostCommand('delete_post_permanently', { id: pending.id })
      : runPostCommand('empty_trash'));
  };

  const matches = (p: Post, f: FilterId) => {
    switch (f) {
      // The trash is its own listing, so no active post is ever in it.
      case 'trash':
        return false;
      case 'all':
        return true;
      // Everything with a publication still to come, including the ones whose
      // time has passed — an overdue post belongs on the screen its owner is
      // looking at for it.
      case 'scheduled':
        return p.schedule?.state === 'scheduled' || p.schedule?.state === 'overdue';
      // Both kinds of failure, because the row badges both as Failed. A
      // publication the Worker could not carry out is the one that most needs
      // looking at — it happened while nobody was watching — and leaving it out
      // of this tab hides it in exactly the place somebody would come looking.
      case 'failed':
        return p.sync === 'sync_failed' || p.schedule?.state === 'failed';
      // A published post whose local version has not been published yet. Drafts
      // are excluded: everything about a draft is unpublished, so listing them
      // here would bury the posts where the distinction actually matters.
      case 'edited':
        return p.status === 'published' && p.sync === 'modified';
      case 'conflict':
        return p.sync === 'conflict';
      default:
        return p.status === f;
    }
  };

  useEffect(() => {
    setTagFilter(params.get('tag'));
  }, [params]);

  const searchMatches = (p: Post) => {
    const q = search.toLowerCase();
    // The tag is lowered like the title is. Tags are stored as typed —
    // `tags_to_json` only trims — so a `Cloudflare` tag was unreachable by
    // search whichever case was typed: the query had already been lowered, and
    // the tag had not.
    if (q === '' || p.title.toLowerCase().includes(q) || p.tags.some((t) => t.toLowerCase().includes(q))) {
      return true;
    }
    // The body, once an answer *for this query* has come back. An answer for an
    // earlier query says nothing about this one, so it contributes nothing
    // rather than keeping unrelated posts on screen until the debounce expires.
    if (bodyMatches?.query !== search.trim()) return false;
    return bodyMatches.matched.includes(p.id);
  };

  /// Exact, not case-insensitive like the search box: the Tags screen lists tags
  /// as they are stored, so a filter arriving from it must mean the one that was
  /// clicked rather than everything that looks like it.
  const tagMatches = (p: Post) => tagFilter === null || p.tags.includes(tagFilter);

  const visible = posts.filter((p) => searchMatches(p) && tagMatches(p) && matches(p, filter));
  const visibleTrash = trashed.filter((p) => searchMatches(p) && tagMatches(p));

  /// The selection as it applies to what is actually on screen.
  ///
  /// `selected` is not trusted directly anywhere. Changing tab, search or tag
  /// filter does not clear it, so a post ticked in the trash and then hidden by
  /// a switch to All would otherwise still be there for the library's actions to
  /// find — published, trashed or tagged without ever being visible. Intersecting
  /// with the rows on screen means a hidden post cannot be acted on at all,
  /// while one that is still shown keeps its tick.
  ///
  /// The trash is one listing *or* the other, never both: it renders under its
  /// own branch below, so unioning the two sets here would have kept a ticked
  /// trashed post live after a switch to All — the exact case the paragraph
  /// above says cannot happen.
  const onScreen = new Set((filter === 'trash' ? visibleTrash : visible).map((p) => p.id));
  const actionable = [...selected].filter((id) => onScreen.has(id));

  const tabs: { id: FilterId; label: string; count: number }[] = (
    ['all', 'published', 'edited', 'conflict', 'draft', 'scheduled', 'failed', 'trash'] as const
  ).map((id) => ({
    id,
    label: {
      all: 'All',
      published: 'Published',
      edited: 'Edited',
      conflict: 'Conflicts',
      draft: 'Drafts',
      scheduled: 'Scheduled',
      failed: 'Failed',
      trash: 'Trash',
    }[id],
    count:
      id === 'trash' ? trashed.filter(tagMatches).length : posts.filter((p) => tagMatches(p) && matches(p, id)).length,
  }));

  return (
    <main className='flex-1 overflow-y-auto p-6'>
      {/* The breadcrumb names the page visually; this names it for outline navigation. */}
      <h1 className='sr-only'>Posts</h1>
      <div className='space-y-4 w-full'>
        {/* Toolbar. Wraps rather than overflows: with eight tabs, the trash
            view's extra button and enlarged text it can outgrow one row. */}
        <div className='flex flex-wrap items-center justify-between gap-x-4 gap-y-2'>
          {/* Left: tabs + search */}
          <div className='flex flex-1 min-w-0 items-center gap-3'>
            {/* Segmented tabs */}
            <Tabs value={filter} onValueChange={(v) => setFilter(v as FilterId)}>
              <TabsList className='h-[32px] gap-px rounded-[7px] border border-border bg-muted p-[3px]'>
                {tabs.map(({ id, label, count }) => (
                  <TabsTrigger
                    key={id}
                    value={id}
                    className='h-[26px] gap-1.5 rounded-[5px] px-3 text-meta font-semibold text-muted-foreground data-active:text-foreground'
                  >
                    {label}
                    <span className='text-label font-bold tabular-nums text-muted-foreground'>{count}</span>
                  </TabsTrigger>
                ))}
              </TabsList>
            </Tabs>

            {/* Search */}
            <div className='relative flex-1 min-w-[180px] max-w-[280px]'>
              <Search
                size={13}
                strokeWidth={1.8}
                className='absolute left-[9px] top-1/2 -translate-y-1/2 z-10 text-muted-foreground pointer-events-none'
              />
              <Input
                type='text'
                placeholder='Search posts, tags, text…'
                aria-label='Search posts'
                value={search}
                onChange={(e) => setSearch(e.target.value)}
                className='h-[30px] w-full pl-[28px] pr-[26px] text-meta rounded-[6px] border-border bg-muted/50'
              />
              {/* While this is spinning, the rows are matched on titles and tags
                  alone — so an empty list is not yet an answer about the text. */}
              {bodySearching && (
                <Loader2
                  size={12}
                  strokeWidth={2}
                  aria-label='Searching post text'
                  className='absolute right-[9px] top-1/2 -translate-y-1/2 animate-spin text-muted-foreground'
                />
              )}
            </div>
          </div>

          {/* Right: CTAs */}
          <div className='flex items-center gap-2 shrink-0'>
            {/* Empty trash — only where it applies, and only when there is
                something to empty. */}
            {filter === 'trash' && trashed.length > 0 && (
              <Button
                variant='outline'
                size='sm'
                onClick={() => setPendingDelete({ kind: 'all', count: trashed.length })}
                className='h-[30px] px-3 gap-[6px] rounded-[6px] text-ui font-semibold text-destructive border-destructive/30 hover:bg-destructive/10'
              >
                <Trash2 size={13} strokeWidth={2} />
                Empty trash
              </Button>
            )}

            {/* Import Article */}
            <Button
              variant='outline'
              size='sm'
              onClick={handleImportArticle}
              disabled={importStatus.kind === 'loading'}
              className='h-[30px] px-3 gap-[6px] rounded-[6px] text-ui font-semibold text-body'
            >
              <Import size={13} strokeWidth={2} />
              {importStatus.kind === 'loading' ? 'Importing…' : 'Import Article'}
            </Button>

            {/* New Post */}
            <Button
              asChild
              size='sm'
              className='h-[30px] px-3 gap-[6px] rounded-[6px] text-ui font-semibold shadow-[0_1px_2px_rgba(0,0,0,0.12)] hover:shadow-[0_2px_8px_rgba(0,0,0,0.18)] dark:hover:shadow-[0_2px_8px_rgba(0,0,0,0.5)]'
            >
              <Link href='/posts/new'>
                <Plus size={13} strokeWidth={2.5} />
                New Post
              </Link>
            </Button>
          </div>
        </div>

        {/* A refused or failed trash action. Given its own banner because these
            are refusals somebody needs to read and act on — a post that cannot
            be trashed until its scheduled publication is cancelled, say. */}
        {actionError && (
          <Alert className='items-center rounded-[6px] px-3 py-2 border-destructive/30 bg-destructive/10 text-destructive'>
            <AlertDescription className='text-meta font-medium text-destructive'>{actionError}</AlertDescription>
          </Alert>
        )}

        {/* Upload feedback banner */}
        {importStatus.kind !== 'idle' &&
          importStatus.kind !== 'loading' &&
          (importStatus.kind === 'success' ? (
            <Alert className='items-center rounded-[6px] px-3 py-2 border-success/30 bg-success/10 text-success'>
              <CheckCircle2 size={13} strokeWidth={2} className='size-3.5' />
              <AlertDescription className='text-meta font-medium text-success'>
                <span className='font-semibold'>&ldquo;{importStatus.title}&rdquo;</span> imported as a draft. Publish
                it to send it to the cloud.
              </AlertDescription>
            </Alert>
          ) : (
            <Alert className='items-center rounded-[6px] px-3 py-2 border-destructive/30 bg-destructive/10 text-destructive'>
              <AlertDescription className='text-meta font-medium text-destructive'>
                <span className='font-bold'>Error:</span> {importStatus.message}
              </AlertDescription>
            </Alert>
          ))}

        {/* The half of the answer that would otherwise be silent. An empty
            result over unsearched posts is not "not found" — it is "not
            looked", and only this says which. */}
        {bodyMatches && bodyMatches.query === search.trim() && bodyMatches.unsearched.length > 0 && (
          <Alert className='items-center rounded-[6px] px-3 py-2 border-border bg-muted/50'>
            <AlertDescription className='flex flex-wrap items-center gap-x-1.5 gap-y-1 text-meta text-body'>
              <span>
                {bodyMatches.unsearched.length} post{bodyMatches.unsearched.length === 1 ? '' : 's'} could not be
                searched
                {bodyMatches.unsearched.length <= 3 && (
                  <>
                    {' — '}
                    {bodyMatches.unsearched.map((u, i) => (
                      <span key={u.id}>
                        {i > 0 && ', '}
                        <span className='font-medium text-body'>{u.title}</span> ({UNSEARCHED_REASON[u.reason]})
                      </span>
                    ))}
                  </>
                )}
                .
              </span>
              <button
                type='button'
                onClick={() => void fillSearchGaps()}
                disabled={fillingGaps}
                className='rounded-[4px] px-1.5 py-0.5 font-medium text-body underline underline-offset-2 transition-colors hover:bg-muted disabled:opacity-60'
              >
                {fillingGaps ? 'Fetching…' : 'Fetch them and search again'}
              </button>
            </AlertDescription>
          </Alert>
        )}

        <BulkActions
          selected={actionable}
          onClear={() => setSelected(new Set())}
          onRun={runBulk}
          inTrash={filter === 'trash'}
        />

        {tagFilter !== null && (
          <div className='flex items-center gap-2 text-meta text-body'>
            <span>Tagged</span>
            <Badge
              variant='outline'
              className='h-auto rounded-[4px] border-border bg-muted px-[6px] py-[2px] font-mono text-label font-semibold text-body'
            >
              {tagFilter}
            </Badge>
            <button
              type='button'
              onClick={() => {
                setTagFilter(null);
                // Take it out of the URL too, or a reload puts it back.
                router.replace('/posts');
              }}
              className='rounded-[4px] px-1.5 py-0.5 text-label font-medium underline underline-offset-2 transition-colors hover:bg-muted'
            >
              Clear
            </button>
          </div>
        )}

        {exportStatus.kind === 'success' && (
          <Alert className='items-center rounded-[6px] px-3 py-2 border-success/30 bg-success/10 text-success'>
            <CheckCircle2 size={13} strokeWidth={2} className='size-3.5' />
            <AlertDescription className='text-meta font-medium text-success'>
              <span className='font-semibold'>&ldquo;{exportStatus.slug}&rdquo;</span> written to{' '}
              <span className='font-mono text-label'>{exportStatus.path}</span>.
              {/* The file and the blog disagree, and only the app knows it. */}
              {exportStatus.unpublishedEdits &&
                ' It includes edits that have not been published, so it is ahead of what readers are being served.'}
            </AlertDescription>
          </Alert>
        )}

        {exportStatus.kind === 'error' && (
          <Alert className='items-center rounded-[6px] px-3 py-2 border-destructive/30 bg-destructive/10 text-destructive'>
            <AlertDescription className='text-meta font-medium text-destructive'>
              <span className='font-bold'>Export failed:</span> {exportStatus.message}
            </AlertDescription>
          </Alert>
        )}

        {/* Trash — its own listing, with its own actions. Deliberately not a
            filter over the table below: these posts are excluded from
            `list_posts` by the backend, which is what keeps them out of every
            other screen as well. */}
        {filter === 'trash' ? (
          <div className='rounded-[8px] border border-border overflow-hidden'>
            <div className='grid grid-cols-[1fr_auto] sm:grid-cols-[1fr_120px_90px_100px_180px] gap-0 border-b border-border bg-muted/50 px-4 py-[8px]'>
              {['Title', 'Tags', 'Status', 'Trashed', ''].map((h, i) => (
                <span
                  key={h || 'actions'}
                  className={cn(
                    'text-label font-bold uppercase tracking-[0.1em] text-muted-foreground',
                    i === 1 || i === 2 || i === 3 ? 'hidden sm:block' : '',
                    i === 4 ? 'hidden sm:block text-right' : '',
                  )}
                >
                  {h}
                </span>
              ))}
            </div>

            <div className='bg-card divide-y divide-border/60'>
              {visibleTrash.map((post) => (
                <div
                  key={post.id}
                  className='grid grid-cols-[1fr_auto] sm:grid-cols-[1fr_120px_90px_100px_180px] items-center gap-0 px-4 py-[10px]'
                >
                  <div className='flex items-center gap-2.5 min-w-0 pr-4'>
                    <Checkbox
                      checked={selected.has(post.id)}
                      onCheckedChange={() => toggleSelected(post.id)}
                      aria-label={`Select ${post.title}`}
                      className='cursor-pointer'
                    />
                    <span className='text-ui font-medium text-muted-foreground truncate line-through decoration-muted-foreground/40'>
                      {post.title}
                    </span>
                  </div>

                  <div className='hidden sm:flex gap-1 flex-wrap'>
                    {post.tags.map((t) => (
                      <Badge
                        key={t}
                        variant='outline'
                        className='h-auto px-[6px] py-[2px] rounded-[4px] text-label font-mono font-semibold bg-muted border-border text-muted-foreground'
                      >
                        {t}
                      </Badge>
                    ))}
                  </div>

                  {/* Trashing is local. A post that was live is still live, and
                      saying so here is the whole reason this column exists. */}
                  <span
                    title={
                      post.live
                        ? 'Still published on the blog — deleting it here does not take it down'
                        : 'Never published'
                    }
                    className={cn(
                      'hidden sm:block text-label font-medium',
                      post.live ? 'text-warning' : 'text-muted-foreground',
                    )}
                  >
                    {post.live ? 'Still live' : 'Draft'}
                  </span>

                  <span className='hidden sm:block text-label font-mono tracking-tight text-muted-foreground'>
                    {post.trashedAt}
                  </span>

                  <div className='flex items-center justify-end gap-1.5'>
                    <Button
                      variant='outline'
                      size='sm'
                      onClick={() => void runPostCommand('restore_post', { id: post.id })}
                      aria-label={`Restore ${post.title}`}
                      className='h-[26px] px-2 gap-1.5 rounded-[5px] text-meta font-semibold text-body'
                    >
                      <RotateCcw size={12} strokeWidth={2} />
                      Restore
                    </Button>
                    <Button
                      variant='ghost'
                      size='sm'
                      onClick={() => setPendingDelete({ kind: 'one', id: post.id, title: post.title })}
                      aria-label={`Delete forever: ${post.title}`}
                      className='h-[26px] px-2 rounded-[5px] text-meta font-semibold text-muted-foreground hover:text-destructive'
                    >
                      Delete forever
                    </Button>
                  </div>
                </div>
              ))}
            </div>

            {visibleTrash.length === 0 && (
              <div className='bg-card py-16 text-center'>
                <p className='text-ui text-muted-foreground'>
                  {loading
                    ? 'Loading trash…'
                    : trashed.length === 0
                      ? 'The trash is empty. Deleted posts wait here until you delete them for good.'
                      : 'Nothing in the trash matches this search.'}
                </p>
              </div>
            )}
          </div>
        ) : (
          <>
            {/* Table */}
            <div className='rounded-[8px] border border-border overflow-hidden'>
              {/* Head */}
              <div className='grid grid-cols-[1fr_auto_auto_auto] sm:grid-cols-[1fr_120px_90px_100px_80px] gap-0 border-b border-border bg-muted/50 px-4 py-[8px]'>
                {['Title', 'Tags', 'Status', 'Date', 'Views'].map((h, i) => (
                  <span
                    key={h}
                    className={cn(
                      'text-label font-bold uppercase tracking-[0.1em] text-muted-foreground',
                      i === 4 ? 'text-right hidden sm:block' : '',
                      i === 1 ? 'hidden sm:block' : '',
                      i === 3 ? 'hidden sm:block' : '',
                    )}
                  >
                    {h}
                  </span>
                ))}
              </div>

              {/* Rows */}
              <div className='bg-card divide-y divide-border/60'>
                {visible.map((post) => (
                  // Not a button itself: the row holds a checkbox and three
                  // buttons of its own, and a button containing controls is
                  // read out as one flattened label with them lost inside it.
                  // The title is the row's keyboard and screen-reader way in;
                  // a click anywhere else on the row is a mouse convenience.
                  <div
                    key={post.id}
                    onClick={(e) => {
                      // A click on one of the row's own controls belongs to that
                      // control. The title link navigates by itself, and pushing
                      // here as well would put the editor in the history twice.
                      if ((e.target as HTMLElement).closest('a, button, input')) return;
                      router.push(`/posts/edit?id=${post.id}`);
                    }}
                    className='group grid grid-cols-[1fr_auto_auto_auto] sm:grid-cols-[1fr_120px_90px_100px_80px] items-center gap-0 px-4 py-[10px] cursor-pointer hover:bg-muted/50 transition-colors duration-100'
                  >
                    <div className='flex items-center gap-2.5 min-w-0 pr-4'>
                      {/* Its own click target: ticking a row must not open it,
                          and the row's handler is on the whole grid. */}
                      <Checkbox
                        checked={selected.has(post.id)}
                        onCheckedChange={() => toggleSelected(post.id)}
                        onClick={(e) => e.stopPropagation()}
                        aria-label={`Select ${post.title}`}
                        className='cursor-pointer'
                      />
                      <StatusDot status={displayStatus(post)} />
                      <Link
                        href={`/posts/edit?id=${post.id}`}
                        className='text-ui font-medium text-foreground truncate rounded-[3px] outline-none group-hover:text-foreground focus-visible:ring-3 focus-visible:ring-ring/50 transition-colors duration-100'
                      >
                        {post.title}
                      </Link>
                    </div>

                    <div className='hidden sm:flex gap-1 flex-wrap'>
                      {post.tags.map((t) => (
                        <Badge
                          key={t}
                          variant='outline'
                          className='h-auto px-[6px] py-[2px] rounded-[4px] text-label font-mono font-semibold bg-muted border-border text-muted-foreground'
                        >
                          {t}
                        </Badge>
                      ))}
                    </div>

                    <div>
                      <StatusPill status={displayStatus(post)} />
                    </div>

                    {/* When it was written, or — for a post with a publication
                        still to come — when that is, which is the date somebody
                        looking at this row actually wants. */}
                    {post.schedule?.state === 'scheduled' || post.schedule?.state === 'overdue' ? (
                      <span
                        title={`Publishes ${new Date(post.schedule.publish_at * 1000).toLocaleString()}`}
                        className='hidden sm:block text-label font-mono tracking-tight text-indigo-500 dark:text-indigo-400'
                      >
                        → {formatScheduleTime(post.schedule.publish_at)}
                      </span>
                    ) : (
                      <span className='hidden sm:block text-label font-mono tracking-tight text-muted-foreground'>
                        {post.date}
                      </span>
                    )}

                    <div className='hidden sm:flex items-center justify-end gap-2'>
                      <span className='text-meta font-mono tabular-nums text-muted-foreground'>
                        {post.views !== undefined ? post.views.toLocaleString() : '—'}
                      </span>
                      {/* No confirmation, by the rule the trash button
                      follows: publishing again puts the post back, so nothing
                      here is spent.

                      Withheld from a post whose cloud copy is ahead or in
                      conflict. `unpublish_post` sends this machine's whole row
                      to D1 — title, excerpt, tags, series — so there it would
                      not just take the post down, it would settle the
                      disagreement in local's favour on the way past. Resolve it
                      in the editor first; the button comes back. */}
                      {/* Writes the post out as a `.md` file. Available for
                      every post, published or not: what it exports is what is
                      on this machine. */}
                      <button
                        type='button'
                        aria-label={`Export ${post.title} as Markdown`}
                        title='Export as Markdown'
                        onClick={(e) => {
                          e.stopPropagation();
                          void exportPost(post.id);
                        }}
                        disabled={exportStatus.kind === 'working'}
                        className='shrink-0 p-1 rounded-[4px] text-muted-foreground opacity-0 group-hover:opacity-100 focus-visible:opacity-100 hover:text-body hover:bg-muted disabled:opacity-40 transition-colors'
                      >
                        <Download size={13} strokeWidth={2} />
                      </button>
                      {post.status === 'published' && post.sync !== 'conflict' && post.sync !== 'remote_ahead' && (
                        <button
                          type='button'
                          aria-label={`Unpublish ${post.title}`}
                          title='Unpublish — takes it off the blog; the local copy stays'
                          onClick={(e) => {
                            e.stopPropagation();
                            void runPostCommand('unpublish_post', { postId: post.id });
                          }}
                          className='shrink-0 p-1 rounded-[4px] text-muted-foreground opacity-0 group-hover:opacity-100 focus-visible:opacity-100 hover:text-warning hover:bg-warning/10 transition-colors'
                        >
                          <EyeOff size={13} strokeWidth={2} />
                        </button>
                      )}
                      {/* Deleting moves the post to the trash and nothing else, so
                      it needs no confirmation of its own — the step that cannot
                      be undone is in there, behind one. */}
                      <button
                        type='button'
                        aria-label={`Move ${post.title} to trash`}
                        title='Move to trash'
                        onClick={(e) => {
                          e.stopPropagation();
                          void runPostCommand('trash_post', { id: post.id });
                        }}
                        className='shrink-0 p-1 rounded-[4px] text-muted-foreground opacity-0 group-hover:opacity-100 focus-visible:opacity-100 hover:text-destructive hover:bg-destructive/10 transition-colors'
                      >
                        <Trash2 size={13} strokeWidth={2} />
                      </button>
                    </div>
                  </div>
                ))}
              </div>

              {visible.length === 0 && (
                <div className='bg-card py-16 text-center'>
                  {/* The failure gets its own element with `role='alert'`, so
                      it is announced when it appears rather than replacing the
                      loading message in silence. */}
                  {!loading && loadError ? (
                    <p role='alert' className='text-ui text-muted-foreground'>
                      Failed to load posts: {loadError}
                    </p>
                  ) : (
                    <p className='text-ui text-muted-foreground'>
                      {loading
                        ? 'Loading posts…'
                        : posts.length === 0
                          ? 'No posts yet. Write one with New Post, or pull existing posts from the cloud.'
                          : search.trim()
                            ? `No posts in this view match “${search.trim()}”.`
                            : 'No posts in this view.'}
                    </p>
                  )}
                </div>
              )}
            </div>

            {visible.length > 0 && (
              <p className='text-label text-muted-foreground px-1'>
                {visible.length} of {posts.length} posts
              </p>
            )}
          </>
        )}

        {/* The one action in this app that cannot be walked back. Both wordings
            say what survives it, because "delete" here means the local copy and
            not the article on the blog.

            An alert dialog, so focus starts on Cancel — the safe choice is the
            one a stray Enter lands on — and goes back to the button that asked
            when it closes. */}
        <AlertDialog
          open={pendingDelete !== null}
          onOpenChange={(next) => {
            if (!next) setPendingDelete(null);
          }}
        >
          <AlertDialogContent className='block w-full rounded-[8px] border border-border bg-card p-4 ring-0 data-[size=default]:max-w-[min(440px,calc(100%-3rem))] data-[size=default]:sm:max-w-[min(440px,calc(100%-3rem))]'>
            {shownDelete && (
              <>
                <AlertDialogTitle className='text-ui font-semibold text-foreground'>
                  {shownDelete.kind === 'one'
                    ? `Delete “${shownDelete.title}” forever?`
                    : `Empty the trash — ${shownDelete.count} ${shownDelete.count === 1 ? 'post' : 'posts'}?`}
                </AlertDialogTitle>
                <AlertDialogDescription className='mt-1.5 text-meta leading-[1.6] text-wrap text-muted-foreground md:text-wrap'>
                  This removes the local copy, its Markdown, and its version history. There is no undo. Anything already
                  published stays on the blog until you unpublish it.
                </AlertDialogDescription>
                <div className='mt-4 flex items-center justify-end gap-2'>
                  <AlertDialogCancel
                    variant='outline'
                    size='sm'
                    className='h-[28px] px-3 rounded-[5px] text-meta font-semibold'
                  >
                    Cancel
                  </AlertDialogCancel>
                  <AlertDialogAction
                    variant='destructive'
                    size='sm'
                    onClick={() => void confirmDelete()}
                    className='h-[28px] px-3 rounded-[5px] text-meta font-semibold'
                  >
                    {shownDelete.kind === 'one' ? 'Delete forever' : 'Empty trash'}
                  </AlertDialogAction>
                </div>
              </>
            )}
          </AlertDialogContent>
        </AlertDialog>
      </div>
    </main>
  );
}
