'use client';

import { useCallback, useEffect, useRef, useState } from 'react';
import { FileWarning, Trash2, Upload } from 'lucide-react';
import { useRouter } from 'next/navigation';
import { Button } from '@/components/ui/button';
import { Dialog, DialogClose, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog';
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog';
import { isVideo } from '@/lib/media';
import { cn } from '@/lib/utils';

type MediaItem = {
  key: string;
  name: string;
  size: number;
  src: string;
  isVideo: boolean;
  /// The posts that would break if this object went, and whether that list is
  /// the whole answer.
  ///
  /// The distinction is the point. "Nothing uses this" invites a delete;
  /// "nobody checked" must not be allowed to look like it — and neither must
  /// "two posts use this, and possibly others nobody could check". See
  /// `src-tauri/src/media_usage.rs` for why a library object is matched to a
  /// post by content rather than by name.
  usedBy: Usage;
};

/// Mirrors `UncheckedPost` in `src-tauri/src/media_usage.rs`: a post the survey
/// could not fully account for, and why.
type UncheckedPost = {
  id: number;
  title: string;
  reason: 'body_not_cached' | 'pending_edits' | 'asset_unreadable' | 'body_stale';
};

/// Said plainly, because each reason has a different way out.
const UNCHECKED_REASON: Record<UncheckedPost['reason'], string> = {
  body_not_cached: 'its text is not on this machine; open it once to bring it down',
  pending_edits: 'it is live with unpublished edits, so readers see a version this machine does not have',
  asset_unreadable: 'it points at a staged image that is missing, so one of its references cannot be identified',
  body_stale: 'its text here is older than the published version; open it once to bring the current one down',
};

/// Mirrors `UsingPost` in `src-tauri/src/media_usage.rs`.
type UsingPost = {
  id: number;
  slug: string;
  title: string;
  trashed: boolean;
  published: boolean;
};

function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

/// What an object's usage amounts to: who is known to use it, and whether
/// anybody could have been missed.
///
/// Both halves are needed. Known users are true whatever else was missed, so
/// they are always reported; but a count presented as exact when some post
/// could not be read is a claim nobody checked.
type Usage = { posts: UsingPost[]; complete: boolean };

/// `posts` missing means the object itself could not be hashed — nothing was
/// matched against it at all. `blind` means some post could not be accounted
/// for in full, so any answer here is a floor rather than a total.
function withUsage(posts: UsingPost[] | undefined, blind: boolean): Usage {
  if (posts === undefined) return { posts: [], complete: false };
  return { posts, complete: !blind };
}

/// How an item's usage reads on its tile. Published posts are called out on
/// their own because those are the references readers are being served, and an
/// incomplete answer says so rather than rounding itself up to a fact.
function usageLabel({ posts, complete }: Usage): string {
  if (posts.length === 0) return complete ? 'Not used' : 'Usage unknown';
  const live = posts.filter((p) => p.published && !p.trashed).length;
  const where = `${posts.length} post${posts.length === 1 ? '' : 's'}`;
  const known = live > 0 ? `${where} · ${live} live` : where;
  return complete ? known : `${known}+ · unverified`;
}

export default function MediaPage() {
  const router = useRouter();
  const [items, setItems] = useState<MediaItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  /// The item whose posts are being inspected, and the one whose deletion is
  /// waiting on an answer. Separate because they are different questions: one
  /// is "what uses this", the other "delete it anyway?".
  const [inspecting, setInspecting] = useState<MediaItem | null>(null);
  const [confirming, setConfirming] = useState<MediaItem | null>(null);
  /// The objects being deleted right now. Per object, because deletes run side
  /// by side — an unused tile deletes straight away, with no dialog — and one
  /// object's delete must neither hold up nor close another's confirmation.
  /// The confirmation stays up until *its* delete settles, so a slow R2 call
  /// cannot be dismissed into looking like it never happened.
  const [deleting, setDeleting] = useState<ReadonlySet<string>>(new Set());
  /// The same set, readable at once: state only changes on the next render,
  /// and a double click lands its second press before that.
  const deletingKeys = useRef(new Set<string>());
  /// Where focus goes back to when a dialog closes. Neither dialog has a Radix
  /// trigger — both open from buttons inside a tile — so without this a
  /// keyboard user would be dropped at the top of the document.
  const returnFocus = useRef<HTMLElement | null>(null);
  /// Posts whose Markdown is not on this machine. While there are any, nothing
  /// here can prove an object is unused — see `withUsage`.
  const [uncheckedPosts, setUncheckedPosts] = useState<UncheckedPost[]>([]);

  // Load media from R2 (cached locally by the backend). No-ops in a plain
  // browser (`pnpm dev`), where the Tauri API isn't available.
  const loadMedia = useCallback(async () => {
    const { invoke, isTauri, convertFileSrc } = await import('@tauri-apps/api/core');
    if (!isTauri()) {
      setLoading(false);
      return;
    }
    const { appDataDir, join } = await import('@tauri-apps/api/path');
    try {
      const rows = await invoke<{ key: string; name: string; size: number }[]>('list_media');
      // After `list_media`, which caches every object locally — the survey reads
      // the cache, so asking it first would report a fresh library as unused.
      const report = await invoke<{
        objects: { key: string; posts: UsingPost[] }[];
        unchecked_posts: UncheckedPost[];
      }>('media_usage');
      // A post whose Markdown is not on this machine — pulled from the cloud
      // and never opened — could reference anything, and nothing here can see
      // it. While any exists, "no known users" is not "unused" for *any*
      // object, so the whole library reads as unknown rather than inviting a
      // delete it cannot justify.
      const blind = report.unchecked_posts.length > 0;
      setUncheckedPosts(report.unchecked_posts);
      const usage = new Map<string, UsingPost[]>(report.objects.map((u) => [u.key, u.posts]));
      const base = await appDataDir();
      const resolved = await Promise.all(
        rows.map(async (r) => ({
          ...r,
          // The key doubles as the local-relative cache path.
          src: convertFileSrc(await join(base, r.key)),
          isVideo: isVideo(r.name),
          // `?? null`, never `?? []`: an object the survey did not report on is
          // one it could not read, and saying "not used" for it is the one
          // wrong answer here. The same goes for an empty list while some
          // post's body is unreadable — nothing checked it.
          usedBy: withUsage(usage.get(r.key), blind),
        })),
      );
      setItems(resolved);
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadMedia();
  }, [loadMedia]);

  const handleUpload = async () => {
    const { invoke, isTauri } = await import('@tauri-apps/api/core');
    if (!isTauri()) return;
    setBusy(true);
    try {
      await invoke('upload_media');
      await loadMedia();
    } catch (e) {
      const msg = String(e);
      if (msg !== 'cancelled') setError(msg);
    } finally {
      setBusy(false);
    }
  };

  /// Delete an object. `force` is the answer to the warning, and is only ever
  /// true on the path that showed one — the backend refuses a referenced object
  /// without it, so a bug here cannot silently break a published post.
  const handleDelete = async (item: MediaItem, force: boolean) => {
    // Claimed before the first await: set after it, as it was, a second press
    // got through while the Tauri module loaded, sent a second delete for an
    // object the first had already removed, and reported that as a failure.
    const inFlight = deletingKeys.current;
    if (inFlight.has(item.key)) return;
    inFlight.add(item.key);
    setDeleting(new Set(inFlight));
    const release = () => {
      inFlight.delete(item.key);
      setDeleting(new Set(inFlight));
    };
    // Settling one delete closes the confirmation only if it is this object's.
    const closeOwnConfirmation = () => setConfirming((c) => (c?.key === item.key ? null : c));
    const { invoke, isTauri } = await import('@tauri-apps/api/core');
    if (!isTauri()) {
      release();
      return;
    }
    try {
      await invoke('delete_media', { key: item.key, force });
      setItems((prev) => prev.filter((i) => i.key !== item.key));
      closeOwnConfirmation();
    } catch (e) {
      setError(String(e));
      closeOwnConfirmation();
    } finally {
      release();
    }
  };

  /// The delete button. An object known to be unused goes straight away;
  /// anything else asks first — either naming the posts that depend on it, or
  /// admitting that it could not be checked.
  const requestDelete = (item: MediaItem) => {
    if (item.usedBy.complete && item.usedBy.posts.length === 0) {
      void handleDelete(item, false);
      return;
    }
    returnFocus.current = document.activeElement as HTMLElement | null;
    setConfirming(item);
  };

  const inspect = (item: MediaItem) => {
    returnFocus.current = document.activeElement as HTMLElement | null;
    setInspecting(item);
  };

  /// Radix sends focus back to a dialog's trigger, and these have none, so this
  /// sends it to the button that opened them instead. A tile deleted in the
  /// meantime has taken that button with it, and there is nothing to return to.
  const restoreFocus = (e: Event) => {
    e.preventDefault();
    if (returnFocus.current?.isConnected) returnFocus.current.focus();
    returnFocus.current = null;
  };

  return (
    <main className='flex-1 overflow-y-auto p-6'>
      <div className='space-y-4 w-full'>
        {/* Toolbar */}
        <div className='flex items-center justify-between gap-4'>
          <div className='flex items-baseline gap-2'>
            <h1 className='text-title font-semibold tracking-tight text-balance text-foreground'>Media Library</h1>
            <span className='text-meta text-muted-foreground'>
              {items.length} {items.length === 1 ? 'file' : 'files'}
            </span>
          </div>
          <Button
            size='sm'
            onClick={handleUpload}
            disabled={busy}
            className='h-[30px] px-3 gap-[6px] rounded-[6px] text-ui font-semibold'
          >
            <Upload size={13} strokeWidth={2} />
            {busy ? 'Uploading…' : 'Upload'}
          </Button>
        </div>

        {/* Why some tiles say "usage unknown". Without this the label looks
            like a bug rather than a fact about what is on this machine. */}
        {uncheckedPosts.length > 0 && (
          <div className='rounded-[6px] px-3 py-2 border border-warning/30 bg-warning/10 text-warning text-meta'>
            <span className='font-medium'>
              {uncheckedPosts.length} {uncheckedPosts.length === 1 ? 'post could' : 'posts could'} not be checked in
              full
            </span>{' '}
            — nothing can be called unused until {uncheckedPosts.length === 1 ? 'it is' : 'they are'} settled.
            <ul className='mt-1.5 space-y-0.5'>
              {uncheckedPosts.map((p) => (
                <li key={p.id}>
                  <span className='font-medium'>{p.title}</span> — {UNCHECKED_REASON[p.reason]}
                </li>
              ))}
            </ul>
          </div>
        )}

        {error && (
          <div
            role='alert'
            className='rounded-[6px] px-3 py-2 border border-destructive/30 bg-destructive/10 text-destructive text-meta font-medium'
          >
            {error}
          </div>
        )}

        {loading ? (
          <p className='py-16 text-center text-ui text-muted-foreground'>Loading media…</p>
        ) : items.length === 0 ? (
          <p className='py-16 text-center text-ui text-muted-foreground'>
            No media yet. Upload an image or video to get started.
          </p>
        ) : (
          <div className='grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-4 xl:grid-cols-5 gap-3'>
            {items.map((item) => (
              <div key={item.key} className='group relative rounded-[8px] border border-border overflow-hidden bg-card'>
                <div className='aspect-square bg-muted/50 flex items-center justify-center overflow-hidden'>
                  {item.isVideo ? (
                    <video src={item.src} muted preload='metadata' className='w-full h-full object-cover' />
                  ) : (
                    // eslint-disable-next-line @next/next/no-img-element
                    <img src={item.src} alt={item.name} loading='lazy' className='w-full h-full object-cover' />
                  )}
                </div>
                <div className='flex items-center justify-between gap-2 px-2.5 py-2 border-t border-border/60'>
                  <div className='min-w-0'>
                    <p className='text-label font-mono text-body truncate'>{item.name}</p>
                    <p className='text-label text-muted-foreground'>
                      {formatSize(item.size)}
                      {' · '}
                      {item.usedBy.posts.length === 0 ? (
                        <span
                          title={
                            item.usedBy.complete
                              ? undefined
                              : 'Something could not be checked, so this cannot be called unused'
                          }
                        >
                          {usageLabel(item.usedBy)}
                        </span>
                      ) : (
                        <button
                          type='button'
                          onClick={() => inspect(item)}
                          title='See which posts use this'
                          className='underline decoration-dotted underline-offset-2 hover:text-body transition-colors'
                        >
                          {usageLabel(item.usedBy)}
                        </button>
                      )}
                    </p>
                  </div>
                  <button
                    type='button'
                    aria-label={`Delete ${item.name}`}
                    onClick={() => requestDelete(item)}
                    className='shrink-0 p-1 rounded-[4px] text-muted-foreground hover:text-destructive hover:bg-destructive/10 transition-colors opacity-0 group-hover:opacity-100 group-focus-within:opacity-100 focus-visible:opacity-100'
                  >
                    <Trash2 size={13} strokeWidth={2} />
                  </button>
                </div>
              </div>
            ))}
          </div>
        )}

        {/* Which posts use an object. Opening one is the point — a name and a
            thumbnail are rarely enough to remember what an image was for. */}
        <Dialog
          open={inspecting !== null}
          onOpenChange={(open) => {
            if (!open) setInspecting(null);
          }}
        >
          {inspecting && (
            <DialogContent
              showCloseButton={false}
              onCloseAutoFocus={restoreFocus}
              className='flex max-h-[70vh] flex-col gap-0 rounded-[8px] border border-border bg-card p-0 ring-0 sm:max-w-[520px]'
            >
              <div className='border-b border-border/60 px-4 py-3'>
                <DialogTitle className='text-ui leading-normal font-semibold text-foreground'>
                  Used by {inspecting.usedBy.posts.length}
                  {inspecting.usedBy.complete ? '' : ' or more'}{' '}
                  {inspecting.usedBy.posts.length === 1 && inspecting.usedBy.complete ? 'post' : 'posts'}
                </DialogTitle>
                <DialogDescription className='mt-0.5 font-mono text-label text-muted-foreground'>
                  {inspecting.name}
                </DialogDescription>
              </div>
              <ul className='min-h-0 flex-1 overflow-y-auto p-2'>
                {inspecting.usedBy.posts.map((post) => (
                  <li key={post.id}>
                    {/* A trashed post is still a reference, and still worth
                        listing — but the editor refuses to open one, so
                        offering it as a link would lead to an error message
                        rather than to the post. It is shown, not offered. */}
                    <button
                      type='button'
                      disabled={post.trashed}
                      title={
                        post.trashed
                          ? 'This post is in the trash. Restore it from Posts → Trash to open it.'
                          : undefined
                      }
                      onClick={() => router.push(`/posts/edit?id=${post.id}`)}
                      className='flex w-full items-baseline justify-between gap-3 rounded-[6px] px-2.5 py-2 text-left transition-colors enabled:hover:bg-muted/50 enabled:active:scale-[0.99] disabled:cursor-default'
                    >
                      <span
                        className={cn(
                          'truncate text-meta font-medium',
                          post.trashed
                            ? 'text-muted-foreground line-through decoration-muted-foreground/40'
                            : 'text-body',
                        )}
                      >
                        {post.title}
                      </span>
                      <span className='shrink-0 text-label font-semibold uppercase tracking-[0.08em] text-muted-foreground'>
                        {post.trashed ? 'In trash' : post.published ? 'Live' : 'Draft'}
                      </span>
                    </button>
                  </li>
                ))}
              </ul>
              <div className='flex justify-end border-t border-border/60 px-4 py-2.5'>
                <DialogClose asChild>
                  <Button variant='outline' size='sm' className='h-[28px] px-3 rounded-[5px] text-meta font-semibold'>
                    Close
                  </Button>
                </DialogClose>
              </div>
            </DialogContent>
          )}
        </Dialog>

        {/* Deleting something a post still points at. The backend refuses this
            without `force`, so this dialog is the only way through — which is
            what makes the warning a gate rather than a courtesy. */}
        <AlertDialog
          open={confirming !== null}
          onOpenChange={(open) => {
            if (!open && !(confirming && deleting.has(confirming.key))) setConfirming(null);
          }}
        >
          {confirming && (
            <AlertDialogContent
              onCloseAutoFocus={restoreFocus}
              className='gap-0 rounded-[8px] border border-border bg-card p-4 ring-0 data-[size=default]:max-w-[480px] data-[size=default]:sm:max-w-[480px]'
            >
              <AlertDialogTitle className='flex items-center gap-2 text-ui font-semibold text-foreground'>
                <FileWarning size={14} strokeWidth={2} className='text-warning' />
                {confirming.usedBy.posts.length === 0
                  ? 'Nobody checked whether this is used'
                  : `${confirming.usedBy.posts.length}${confirming.usedBy.complete ? '' : ' or more'} ${
                      confirming.usedBy.posts.length === 1 && confirming.usedBy.complete ? 'post uses' : 'posts use'
                    } this image`}
              </AlertDialogTitle>
              <AlertDialogDescription className='mt-1.5 text-meta leading-[1.6] text-pretty text-muted-foreground'>
                {confirming.usedBy.posts.length === 0
                  ? 'Either this object is not cached on this machine, or some post could not be accounted for in full — so there was nothing conclusive to match against. It may well be in use. The banner above says what would settle it.'
                  : confirming.usedBy.complete
                    ? 'Deleting it removes the object from R2. Any published post pointing at it will show a broken image to readers until it is replaced.'
                    : 'Deleting it removes the object from R2, and these are only the posts that could be checked — others may reference it too. Any published post pointing at it will show a broken image to readers until it is replaced.'}
              </AlertDialogDescription>
              {confirming.usedBy.posts.length > 0 && (
                <ul className='mt-3 max-h-[180px] overflow-y-auto rounded-[6px] border border-border'>
                  {confirming.usedBy.posts.map((post) => (
                    <li
                      key={post.id}
                      className='flex items-baseline justify-between gap-3 border-b border-border/60 px-2.5 py-1.5 last:border-b-0'
                    >
                      <span className='truncate text-meta text-body'>{post.title}</span>
                      <span className='shrink-0 text-label font-semibold uppercase tracking-[0.08em] text-muted-foreground'>
                        {post.trashed ? 'In trash' : post.published ? 'Live' : 'Draft'}
                      </span>
                    </li>
                  ))}
                </ul>
              )}
              {/* An alert dialog opens on its Cancel, so the destructive answer
                  always takes a deliberate move, and it is not dismissed by a
                  stray click outside. */}
              <div className='mt-4 flex items-center justify-end gap-2'>
                <AlertDialogCancel
                  size='sm'
                  disabled={deleting.has(confirming.key)}
                  className='h-[28px] px-3 rounded-[5px] text-meta font-semibold'
                >
                  Keep it
                </AlertDialogCancel>
                <AlertDialogAction
                  variant='destructive'
                  size='sm'
                  disabled={deleting.has(confirming.key)}
                  onClick={(e) => {
                    // Held open until the delete settles: `handleDelete`
                    // closes it either way, and says so if it failed.
                    e.preventDefault();
                    void handleDelete(confirming, true);
                  }}
                  className='h-[28px] px-3 rounded-[5px] text-meta font-semibold'
                >
                  {deleting.has(confirming.key) ? 'Deleting…' : 'Delete anyway'}
                </AlertDialogAction>
              </div>
            </AlertDialogContent>
          )}
        </AlertDialog>
      </div>
    </main>
  );
}
