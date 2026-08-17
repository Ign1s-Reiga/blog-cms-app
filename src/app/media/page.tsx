'use client';

import { useCallback, useEffect, useState } from 'react';
import { FileWarning, Trash2, Upload } from 'lucide-react';
import { useRouter } from 'next/navigation';
import { Button } from '@/components/ui/button';
import { cn } from '@/lib/utils';

type MediaItem = {
  key: string;
  name: string;
  size: number;
  src: string;
  isVideo: boolean;
  /// The posts that would break if this object went, or `null` when the survey
  /// could not answer for this object — it is not cached locally, so there are
  /// no bytes to match posts against.
  ///
  /// The distinction is the point. "Nothing uses this" invites a delete;
  /// "nobody checked" must not be allowed to look like it. See
  /// `src-tauri/src/media_usage.rs` for why a library object is matched to a
  /// post by content rather than by name.
  usedBy: UsingPost[] | null;
};

/// Mirrors `UncheckedPost` in `src-tauri/src/media_usage.rs`: a post the survey
/// could not fully account for, and why.
type UncheckedPost = {
  id: number;
  title: string;
  reason: 'body_not_cached' | 'pending_edits' | 'asset_unreadable';
};

/// Said plainly, because each reason has a different way out.
const UNCHECKED_REASON: Record<UncheckedPost['reason'], string> = {
  body_not_cached: 'its text is not on this machine; open it once to bring it down',
  pending_edits: 'it is live with unpublished edits, so readers see a version this machine does not have',
  asset_unreadable: 'it points at a staged image that is missing, so one of its references cannot be identified',
};

/// Mirrors `UsingPost` in `src-tauri/src/media_usage.rs`.
type UsingPost = {
  id: number;
  slug: string;
  title: string;
  trashed: boolean;
  published: boolean;
};

const VIDEO_EXT = /\.(?:mp4|webm|mov)$/i;

function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

/// What an object's usage amounts to, given what the survey could and could not
/// read.
///
/// `posts` missing means the object itself could not be hashed. An empty list
/// while some post's Markdown is unreadable means nothing checked it — which is
/// the same amount of knowledge, and must not be dressed up as "unused". Known
/// users are still reported as such: they are true whatever else was missed.
function withUsage(posts: UsingPost[] | undefined, blind: boolean): UsingPost[] | null {
  if (posts === undefined) return null;
  if (posts.length === 0 && blind) return null;
  return posts;
}

/// How an item's usage reads on its tile. Published posts are called out on
/// their own because those are the references readers are being served.
function usageLabel(posts: UsingPost[] | null): string {
  if (posts === null) return 'Usage unknown';
  if (posts.length === 0) return 'Not used';
  const live = posts.filter((p) => p.published && !p.trashed).length;
  const where = `${posts.length} post${posts.length === 1 ? '' : 's'}`;
  return live > 0 ? `${where} · ${live} live` : where;
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
          isVideo: VIDEO_EXT.test(r.name),
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
    const { invoke, isTauri } = await import('@tauri-apps/api/core');
    if (!isTauri()) return;
    try {
      await invoke('delete_media', { key: item.key, force });
      setItems((prev) => prev.filter((i) => i.key !== item.key));
      setConfirming(null);
    } catch (e) {
      setError(String(e));
      setConfirming(null);
    }
  };

  /// The delete button. An object known to be unused goes straight away;
  /// anything else asks first — either naming the posts that depend on it, or
  /// admitting that it could not be checked.
  const requestDelete = (item: MediaItem) => {
    if (item.usedBy !== null && item.usedBy.length === 0) {
      void handleDelete(item, false);
      return;
    }
    setConfirming(item);
  };

  return (
    <main className='flex-1 overflow-y-auto p-6'>
      <div className='space-y-4 w-full'>
        {/* Toolbar */}
        <div className='flex items-center justify-between gap-4'>
          <div className='flex items-baseline gap-2'>
            <h1 className='text-[15px] font-semibold text-zinc-800 dark:text-zinc-200'>Media Library</h1>
            <span className='text-[12px] text-zinc-400 dark:text-zinc-600'>
              {items.length} {items.length === 1 ? 'file' : 'files'}
            </span>
          </div>
          <Button
            size='sm'
            onClick={handleUpload}
            disabled={busy}
            className='h-[30px] px-3 gap-[6px] rounded-[6px] text-[13px] font-semibold'
          >
            <Upload size={13} strokeWidth={2} />
            {busy ? 'Uploading…' : 'Upload'}
          </Button>
        </div>

        {/* Why some tiles say "usage unknown". Without this the label looks
            like a bug rather than a fact about what is on this machine. */}
        {uncheckedPosts.length > 0 && (
          <div className='rounded-[6px] px-3 py-2 border border-amber-200 bg-amber-50 text-amber-800 dark:border-amber-500/20 dark:bg-amber-500/[0.08] dark:text-amber-400 text-[12px]'>
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
          <div className='rounded-[6px] px-3 py-2 border border-red-200 bg-red-50 text-red-700 dark:border-red-500/20 dark:bg-red-500/[0.08] dark:text-red-400 text-[12px] font-medium'>
            {error}
          </div>
        )}

        {loading ? (
          <p className='py-16 text-center text-[13px] text-zinc-400 dark:text-zinc-600'>Loading media…</p>
        ) : items.length === 0 ? (
          <p className='py-16 text-center text-[13px] text-zinc-400 dark:text-zinc-600'>
            No media yet. Upload an image or video to get started.
          </p>
        ) : (
          <div className='grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-4 xl:grid-cols-5 gap-3'>
            {items.map((item) => (
              <div
                key={item.key}
                className='group relative rounded-[8px] border border-zinc-200 dark:border-white/[0.07] overflow-hidden bg-white dark:bg-[#161616]'
              >
                <div className='aspect-square bg-zinc-50 dark:bg-white/[0.02] flex items-center justify-center overflow-hidden'>
                  {item.isVideo ? (
                    <video src={item.src} muted preload='metadata' className='w-full h-full object-cover' />
                  ) : (
                    // eslint-disable-next-line @next/next/no-img-element
                    <img src={item.src} alt={item.name} loading='lazy' className='w-full h-full object-cover' />
                  )}
                </div>
                <div className='flex items-center justify-between gap-2 px-2.5 py-2 border-t border-zinc-100 dark:border-white/[0.05]'>
                  <div className='min-w-0'>
                    <p className='text-[11px] font-mono text-zinc-600 dark:text-zinc-400 truncate'>{item.name}</p>
                    <p className='text-[10px] text-zinc-400 dark:text-zinc-600'>
                      {formatSize(item.size)}
                      {' · '}
                      {item.usedBy === null ? (
                        <span title='This object is not cached on this machine, so its usage could not be checked'>
                          {usageLabel(null)}
                        </span>
                      ) : item.usedBy.length === 0 ? (
                        <span>Not used</span>
                      ) : (
                        <button
                          type='button'
                          onClick={() => setInspecting(item)}
                          title='See which posts use this'
                          className='underline decoration-dotted underline-offset-2 hover:text-zinc-600 dark:hover:text-zinc-400 transition-colors'
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
                    className='shrink-0 p-1 rounded-[4px] text-zinc-400 hover:text-red-600 dark:hover:text-red-400 hover:bg-red-50 dark:hover:bg-red-500/[0.1] transition-colors opacity-0 group-hover:opacity-100'
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
        {inspecting && (
          <div
            role='dialog'
            aria-modal='true'
            aria-label={`Posts using ${inspecting.name}`}
            className='fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-6'
            onPointerDown={(e) => {
              if (e.target === e.currentTarget) setInspecting(null);
            }}
          >
            <div className='flex max-h-[70vh] w-full max-w-[520px] flex-col rounded-[8px] border border-zinc-200 dark:border-white/[0.08] bg-white dark:bg-[#161616]'>
              <div className='border-b border-zinc-100 dark:border-white/[0.05] px-4 py-3'>
                <h2 className='text-[13px] font-semibold text-zinc-800 dark:text-zinc-200'>
                  Used by {(inspecting.usedBy ?? []).length} {(inspecting.usedBy ?? []).length === 1 ? 'post' : 'posts'}
                </h2>
                <p className='mt-0.5 font-mono text-[11px] text-zinc-400 dark:text-zinc-600'>{inspecting.name}</p>
              </div>
              <ul className='min-h-0 flex-1 overflow-y-auto p-2'>
                {(inspecting.usedBy ?? []).map((post) => (
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
                      className='flex w-full items-baseline justify-between gap-3 rounded-[6px] px-2.5 py-2 text-left transition-colors enabled:hover:bg-zinc-50 enabled:active:scale-[0.99] disabled:cursor-default dark:enabled:hover:bg-white/[0.03]'
                    >
                      <span
                        className={cn(
                          'truncate text-[12px] font-medium',
                          post.trashed
                            ? 'text-zinc-400 line-through decoration-zinc-300 dark:text-zinc-600 dark:decoration-zinc-700'
                            : 'text-zinc-700 dark:text-zinc-300',
                        )}
                      >
                        {post.title}
                      </span>
                      <span className='shrink-0 text-[10px] font-semibold uppercase tracking-[0.08em] text-zinc-400 dark:text-zinc-600'>
                        {post.trashed ? 'In trash' : post.published ? 'Live' : 'Draft'}
                      </span>
                    </button>
                  </li>
                ))}
              </ul>
              <div className='flex justify-end border-t border-zinc-100 dark:border-white/[0.05] px-4 py-2.5'>
                <Button
                  variant='outline'
                  size='sm'
                  onClick={() => setInspecting(null)}
                  className='h-[28px] px-3 rounded-[5px] text-[12px] font-semibold'
                >
                  Close
                </Button>
              </div>
            </div>
          </div>
        )}

        {/* Deleting something a post still points at. The backend refuses this
            without `force`, so this dialog is the only way through — which is
            what makes the warning a gate rather than a courtesy. */}
        {confirming && (
          <div
            role='dialog'
            aria-modal='true'
            aria-label='Confirm media deletion'
            className='fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-6'
            onPointerDown={(e) => {
              if (e.target === e.currentTarget) setConfirming(null);
            }}
          >
            <div className='w-full max-w-[480px] rounded-[8px] border border-zinc-200 dark:border-white/[0.08] bg-white dark:bg-[#161616] p-4'>
              <h2 className='flex items-center gap-2 text-[13px] font-semibold text-zinc-800 dark:text-zinc-200'>
                <FileWarning size={14} strokeWidth={2} className='text-amber-500' />
                {confirming.usedBy === null
                  ? 'Nobody checked whether this is used'
                  : `${confirming.usedBy.length} ${confirming.usedBy.length === 1 ? 'post uses' : 'posts use'} this image`}
              </h2>
              <p className='mt-1.5 text-[12px] leading-[1.6] text-zinc-500 dark:text-zinc-500'>
                {confirming.usedBy === null
                  ? 'Either this object is not cached on this machine, or some post’s text is not — so there was nothing to match against. It may well be in use. Opening those posts once, or reloading this page, answers the question.'
                  : 'Deleting it removes the object from R2. Any published post pointing at it will show a broken image to readers until it is replaced.'}
              </p>
              {confirming.usedBy !== null && (
                <ul className='mt-3 max-h-[180px] overflow-y-auto rounded-[6px] border border-zinc-200 dark:border-white/[0.07]'>
                  {confirming.usedBy.map((post) => (
                    <li
                      key={post.id}
                      className='flex items-baseline justify-between gap-3 border-b border-zinc-100 px-2.5 py-1.5 last:border-b-0 dark:border-white/[0.04]'
                    >
                      <span className='truncate text-[12px] text-zinc-700 dark:text-zinc-300'>{post.title}</span>
                      <span className='shrink-0 text-[10px] font-semibold uppercase tracking-[0.08em] text-zinc-400 dark:text-zinc-600'>
                        {post.trashed ? 'In trash' : post.published ? 'Live' : 'Draft'}
                      </span>
                    </li>
                  ))}
                </ul>
              )}
              <div className='mt-4 flex items-center justify-end gap-2'>
                <Button
                  variant='outline'
                  size='sm'
                  onClick={() => setConfirming(null)}
                  className='h-[28px] px-3 rounded-[5px] text-[12px] font-semibold'
                >
                  Keep it
                </Button>
                <Button
                  size='sm'
                  onClick={() => void handleDelete(confirming, true)}
                  className='h-[28px] px-3 rounded-[5px] text-[12px] font-semibold bg-red-600 text-white hover:bg-red-700 dark:bg-red-600 dark:hover:bg-red-700'
                >
                  Delete anyway
                </Button>
              </div>
            </div>
          </div>
        )}
      </div>
    </main>
  );
}
