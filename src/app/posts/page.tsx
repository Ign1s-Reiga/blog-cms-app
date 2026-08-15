'use client';

import { useCallback, useEffect, useState } from 'react';
import { CheckCircle2, Import, Plus, Search } from 'lucide-react';
import Link from 'next/link';
import { useRouter } from 'next/navigation';
import { StatusDot, type PostStatus } from '@/components/StatusDot';
import { StatusPill } from '@/components/StatusPill';
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { Input } from '@/components/ui/input';
import { Button } from '@/components/ui/button';
import { Badge } from '@/components/ui/badge';
import { Alert, AlertDescription } from '@/components/ui/alert';
import { onPostsRefreshed } from '@/lib/sync';

// ─── Types ────────────────────────────────────────────────────────────────────

type FilterId = 'all' | 'published' | 'edited' | 'conflict' | 'draft' | 'failed';

type ImportStatus =
  | { kind: 'idle' }
  | { kind: 'loading' }
  | { kind: 'success'; title: string }
  | { kind: 'error'; message: string };

// Row shape the table renders.
type Post = {
  id: number;
  title: string;
  tags: string[];
  status: 'published' | 'draft';
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
  title: string;
  tags: string | null; // JSON-encoded string[]
  published: boolean;
  created_at: number; // Unix seconds
};

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
  return post.status;
}

export default function PostsPage() {
  const router = useRouter();
  const [filter, setFilter] = useState<FilterId>('all');
  const [search, setSearch] = useState('');
  const [importStatus, setImportStatus] = useState<ImportStatus>({ kind: 'idle' });

  const [posts, setPosts] = useState<Post[]>([]);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);

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
      setPosts(rows.map((p) => ({ ...toPost(p), sync: sync.get(p.id) ?? 'clean' })));
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

  const matches = (p: Post, f: FilterId) => {
    switch (f) {
      case 'all':
        return true;
      case 'failed':
        return p.sync === 'sync_failed';
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

  const visible = posts.filter((p) => {
    const q = search.toLowerCase();
    const matchSearch = q === '' || p.title.toLowerCase().includes(q) || p.tags.some((t) => t.includes(q));
    return matchSearch && matches(p, filter);
  });

  const tabs: { id: FilterId; label: string; count: number }[] = (
    ['all', 'published', 'edited', 'conflict', 'draft', 'failed'] as const
  ).map((id) => ({
    id,
    label: {
      all: 'All',
      published: 'Published',
      edited: 'Edited',
      conflict: 'Conflicts',
      draft: 'Drafts',
      failed: 'Failed',
    }[id],
    count: posts.filter((p) => matches(p, id)).length,
  }));

  return (
    <main className='flex-1 overflow-y-auto p-6'>
      <div className='space-y-4 w-full'>
        {/* Toolbar */}
        <div className='flex items-center justify-between gap-4'>
          {/* Left: tabs + search */}
          <div className='flex items-center gap-3'>
            {/* Segmented tabs */}
            <Tabs value={filter} onValueChange={(v) => setFilter(v as FilterId)}>
              <TabsList className='h-[32px] gap-px rounded-[7px] border border-zinc-200 dark:border-white/[0.07] bg-zinc-100 dark:bg-white/[0.04] p-[3px]'>
                {tabs.map(({ id, label, count }) => (
                  <TabsTrigger
                    key={id}
                    value={id}
                    className='h-[26px] gap-1.5 rounded-[5px] px-3 text-[12px] font-semibold text-zinc-500 dark:text-zinc-500 data-active:text-zinc-800 dark:data-active:text-zinc-100'
                  >
                    {label}
                    <span className='text-[10px] font-bold tabular-nums text-zinc-400 dark:text-zinc-500'>{count}</span>
                  </TabsTrigger>
                ))}
              </TabsList>
            </Tabs>

            {/* Search */}
            <div className='relative'>
              <Search
                size={13}
                strokeWidth={1.8}
                className='absolute left-[9px] top-1/2 -translate-y-1/2 z-10 text-zinc-400 dark:text-zinc-600 pointer-events-none'
              />
              <Input
                type='text'
                placeholder='Search posts, tags…'
                value={search}
                onChange={(e) => setSearch(e.target.value)}
                className='h-[30px] w-[200px] pl-[28px] pr-3 text-[12px] rounded-[6px] border-zinc-200 dark:border-white/[0.08] bg-zinc-50 dark:bg-white/[0.04]'
              />
            </div>
          </div>

          {/* Right: CTAs */}
          <div className='flex items-center gap-2 shrink-0'>
            {/* Import Article */}
            <Button
              variant='outline'
              size='sm'
              onClick={handleImportArticle}
              disabled={importStatus.kind === 'loading'}
              className='h-[30px] px-3 gap-[6px] rounded-[6px] text-[13px] font-semibold text-zinc-600 dark:text-zinc-400'
            >
              <Import size={13} strokeWidth={2} />
              {importStatus.kind === 'loading' ? 'Importing…' : 'Import Article'}
            </Button>

            {/* New Post */}
            <Button
              asChild
              size='sm'
              className='h-[30px] px-3 gap-[6px] rounded-[6px] text-[13px] font-semibold shadow-[0_1px_2px_rgba(0,0,0,0.12)] hover:shadow-[0_2px_8px_rgba(0,0,0,0.18)] dark:hover:shadow-[0_2px_8px_rgba(0,0,0,0.5)]'
            >
              <Link href='/posts/new'>
                <Plus size={13} strokeWidth={2.5} />
                New Post
              </Link>
            </Button>
          </div>
        </div>

        {/* Upload feedback banner */}
        {importStatus.kind !== 'idle' &&
          importStatus.kind !== 'loading' &&
          (importStatus.kind === 'success' ? (
            <Alert className='items-center rounded-[6px] px-3 py-2 border-emerald-200 bg-emerald-50 text-emerald-700 dark:border-emerald-500/20 dark:bg-emerald-500/[0.08] dark:text-emerald-400'>
              <CheckCircle2 size={13} strokeWidth={2} className='size-3.5' />
              <AlertDescription className='text-[12px] font-medium text-emerald-700 dark:text-emerald-400'>
                <span className='font-semibold'>&ldquo;{importStatus.title}&rdquo;</span> imported as a draft. Publish
                it to send it to the cloud.
              </AlertDescription>
            </Alert>
          ) : (
            <Alert className='items-center rounded-[6px] px-3 py-2 border-red-200 bg-red-50 text-red-700 dark:border-red-500/20 dark:bg-red-500/[0.08] dark:text-red-400'>
              <AlertDescription className='text-[12px] font-medium text-red-700 dark:text-red-400'>
                <span className='font-bold'>Error:</span> {importStatus.message}
              </AlertDescription>
            </Alert>
          ))}

        {/* Table */}
        <div className='rounded-[8px] border border-zinc-200 dark:border-white/[0.07] overflow-hidden'>
          {/* Head */}
          <div className='grid grid-cols-[1fr_auto_auto_auto] sm:grid-cols-[1fr_120px_90px_100px_80px] gap-0 border-b border-zinc-200 dark:border-white/[0.07] bg-zinc-50 dark:bg-white/[0.02] px-4 py-[8px]'>
            {['Title', 'Tags', 'Status', 'Date', 'Views'].map((h, i) => (
              <span
                key={h}
                className={[
                  'text-[10px] font-bold uppercase tracking-[0.1em] text-zinc-400 dark:text-zinc-600',
                  i === 4 ? 'text-right hidden sm:block' : '',
                  i === 1 ? 'hidden sm:block' : '',
                  i === 3 ? 'hidden sm:block' : '',
                ].join(' ')}
              >
                {h}
              </span>
            ))}
          </div>

          {/* Rows */}
          <div className='bg-white dark:bg-[#161616] divide-y divide-zinc-100 dark:divide-white/[0.04]'>
            {visible.map((post) => (
              <div
                key={post.id}
                role='button'
                tabIndex={0}
                onClick={() => router.push(`/posts/edit?id=${post.id}`)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter' || e.key === ' ') {
                    e.preventDefault();
                    router.push(`/posts/edit?id=${post.id}`);
                  }
                }}
                className='group grid grid-cols-[1fr_auto_auto_auto] sm:grid-cols-[1fr_120px_90px_100px_80px] items-center gap-0 px-4 py-[10px] cursor-pointer hover:bg-zinc-50 dark:hover:bg-white/[0.02] transition-colors duration-100'
              >
                <div className='flex items-center gap-2.5 min-w-0 pr-4'>
                  <StatusDot status={displayStatus(post)} />
                  <span className='text-[13px] font-medium text-zinc-800 dark:text-zinc-200 truncate group-hover:text-zinc-900 dark:group-hover:text-white transition-colors duration-100'>
                    {post.title}
                  </span>
                </div>

                <div className='hidden sm:flex gap-1 flex-wrap'>
                  {post.tags.map((t) => (
                    <Badge
                      key={t}
                      variant='outline'
                      className='h-auto px-[6px] py-[2px] rounded-[4px] text-[10px] font-mono font-semibold bg-zinc-100 dark:bg-white/[0.05] border-zinc-200 dark:border-white/[0.07] text-zinc-500 dark:text-zinc-500'
                    >
                      {t}
                    </Badge>
                  ))}
                </div>

                <div>
                  <StatusPill status={displayStatus(post)} />
                </div>

                <span className='hidden sm:block text-[11px] font-mono tracking-tight text-zinc-400 dark:text-zinc-600'>
                  {post.date}
                </span>

                <span className='hidden sm:block text-right text-[12px] font-mono tabular-nums text-zinc-400 dark:text-zinc-600'>
                  {post.views !== undefined ? post.views.toLocaleString() : '—'}
                </span>
              </div>
            ))}
          </div>

          {visible.length === 0 && (
            <div className='bg-white dark:bg-[#161616] py-16 text-center'>
              <p className='text-[13px] text-zinc-400 dark:text-zinc-600'>
                {loading
                  ? 'Loading posts…'
                  : loadError
                    ? `Failed to load posts: ${loadError}`
                    : posts.length === 0
                      ? 'No posts yet.'
                      : 'No posts match this filter.'}
              </p>
            </div>
          )}
        </div>

        {visible.length > 0 && (
          <p className='text-[11px] text-zinc-400 dark:text-zinc-600 px-1'>
            {visible.length} of {posts.length} posts
          </p>
        )}
      </div>
    </main>
  );
}
