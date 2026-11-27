'use client';

import { useCallback, useEffect, useState } from 'react';
import {
  ArrowUpRight,
  CheckCircle2,
  Clock,
  CloudDownload,
  CloudUpload,
  FilePen,
  FileText,
  Plus,
  TrendingUp,
  Upload,
  type LucideIcon,
} from 'lucide-react';
import Link from 'next/link';
import { toast } from 'sonner';
import { SectionHeader } from '@/components/SectionHeader';
import { StatusDot } from '@/components/StatusDot';
import { AnalyticsCard } from '@/components/AnalyticsCard';
import { Card } from '@/components/ui/card';
import { SYNC_UNAVAILABLE, onPostsRefreshed, pullFromCloud, pushToCloud } from '@/lib/sync';
import { cn } from '@/lib/utils';
import { invoke, isTauri } from '@tauri-apps/api/core';

// ─── Data mapping ─────────────────────────────────────────────────────────────

type Post = {
  id: number;
  title: string;
  tags: string[];
  status: 'published' | 'draft';
  date: string;
  createdAt: number; // Unix seconds
};

// Subset of the `list_posts` command payload we use here.
type BackendPost = {
  id: number;
  title: string;
  tags: string | null; // JSON-encoded string[]
  published: boolean;
  created_at: number;
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
    date: new Date(p.created_at * 1000).toISOString().slice(0, 10),
    createdAt: p.created_at,
  };
}

// ─── Quick action card ────────────────────────────────────────────────────────

const QUICK_CARD_CLASS = [
  'group flex items-start gap-3 p-4 text-left rounded-[8px]',
  'bg-card',
  'border border-border',
  'hover:border-input',
  'hover:shadow-[0_4px_16px_rgba(0,0,0,0.05)] dark:hover:shadow-[0_4px_16px_rgba(0,0,0,0.4)]',
  'active:scale-[0.975] active:translate-y-px active:shadow-none active:border-border active:transition-none',
  'disabled:opacity-60 disabled:cursor-wait disabled:active:scale-100',
  'transition-[border-color,box-shadow,transform] duration-150',
].join(' ');

function QuickActionInner({ Icon, label, desc }: { Icon: LucideIcon; label: string; desc: string }) {
  return (
    <>
      <div className='p-1.75 rounded-md bg-muted/50 border border-border/60 group-hover:bg-muted transition-colors duration-150 shrink-0 mt-px'>
        <Icon size={13} strokeWidth={2} className='text-muted-foreground' />
      </div>
      <div className='flex-1 min-w-0'>
        <p className='text-ui font-semibold text-foreground leading-none'>{label}</p>
        <p className='text-label text-muted-foreground mt-1.25 leading-tight'>{desc}</p>
      </div>
      <ArrowUpRight
        size={13}
        strokeWidth={1.8}
        className='shrink-0 text-muted-foreground group-hover:text-muted-foreground mt-px transition-colors duration-150'
      />
    </>
  );
}

// ─── Page ─────────────────────────────────────────────────────────────────────

export default function DashboardPage() {
  const [posts, setPosts] = useState<Post[]>([]);
  const [loading, setLoading] = useState(true);

  // Load posts from the local SQLite cache via the Tauri backend. No-ops in a
  // plain browser (`pnpm dev`), where the Tauri API isn't available.
  const loadPosts = useCallback(async () => {
    if (!isTauri()) {
      setLoading(false);
      return;
    }
    try {
      // Local cache — refreshed from the cloud on launch and via the refresh button.
      const rows = await invoke<BackendPost[]>('list_posts');
      setPosts(rows.map(toPost));
    } catch (err) {
      console.error('Failed to load posts:', err);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadPosts();
  }, [loadPosts]);

  // Re-read local data after a cloud refresh.
  useEffect(() => onPostsRefreshed(() => void loadPosts()), [loadPosts]);

  // Pull (D1 → local) / push (local → D1) from the quick-action cards.
  const [syncing, setSyncing] = useState<{ pull: boolean; push: boolean }>({ pull: false, push: false });
  const runSync = useCallback(async (which: 'pull' | 'push') => {
    setSyncing((s) => ({ ...s, [which]: true }));
    try {
      const ran = await (which === 'pull' ? pullFromCloud() : pushToCloud());
      // Said out loud because a sync that changed nothing on screen is
      // otherwise indistinguishable from a click that did nothing — and when
      // there was no sync at all, that is said instead of claiming one.
      if (!ran) toast(SYNC_UNAVAILABLE);
      else toast.success(which === 'pull' ? 'Pulled posts from the cloud' : 'Pushed local posts to the cloud');
    } catch (err) {
      console.error(`${which} failed:`, err);
      toast.error(which === 'pull' ? 'Could not pull posts from the cloud' : 'Could not push posts to the cloud', {
        description: String(err),
      });
    } finally {
      setSyncing((s) => ({ ...s, [which]: false }));
    }
  }, []);

  // ── Computed stats ──────────────────────────────────────────────────────────
  const total = posts.length;
  const published = posts.filter((p) => p.status === 'published').length;
  const drafts = total - published;
  const weekAgo = Date.now() / 1000 - 7 * 86_400;
  const thisWeek = posts.filter((p) => p.createdAt >= weekAgo).length;
  const pct = total > 0 ? Math.round((published / total) * 100) : 0;

  const stats = [
    { label: 'Total Posts', value: total, Icon: FileText, delta: `${published} published`, positive: true },
    { label: 'Published', value: published, Icon: CheckCircle2, delta: `${pct}% of total`, positive: true },
    {
      label: 'Drafts',
      value: drafts,
      Icon: FilePen,
      delta: drafts === 0 ? 'All caught up' : 'Pending review',
      positive: drafts === 0,
    },
    { label: 'This Week', value: thisWeek, Icon: Clock, delta: 'New in last 7 days', positive: thisWeek > 0 },
  ];

  const recent = posts.slice(0, 4);

  return (
    <main className='flex-1 overflow-y-auto p-6'>
      {/* The breadcrumb names the page visually; this names it for outline navigation. */}
      <h1 className='sr-only'>Dashboard</h1>
      <div className='space-y-10 w-full'>
        {/* Stats */}
        <section>
          <SectionHeader>Overview</SectionHeader>
          <div className='grid grid-cols-2 lg:grid-cols-4 gap-3'>
            {stats.map(({ label, value, Icon, delta, positive }) => (
              <Card
                key={label}
                className={cn(
                  'group relative flex flex-col justify-between',
                  'p-4 rounded-lg gap-0 ring-0',
                  'bg-card',
                  'border border-border',
                  'hover:border-input',
                  'hover:shadow-[0_4px_20px_rgba(0,0,0,0.06)] dark:hover:shadow-[0_4px_20px_rgba(0,0,0,0.4)]',
                  'transition-[border-color,box-shadow] duration-200',
                )}
              >
                <div className='flex items-center justify-between gap-2'>
                  <span className='text-label font-bold text-muted-foreground uppercase tracking-widest'>{label}</span>
                  <div className='p-1.25 rounded-[5px] bg-muted/50 border border-border/60'>
                    <Icon size={12} strokeWidth={1.8} className='text-muted-foreground' />
                  </div>
                </div>

                <p className='mt-3 text-[28px] font-bold tracking-tight leading-none text-foreground tabular-nums'>
                  {loading ? '—' : value}
                </p>

                <div className='mt-3 pt-3 border-t border-border/60 flex items-center gap-1'>
                  {positive && <TrendingUp size={11} strokeWidth={2} className='text-success shrink-0' />}
                  <span
                    className={cn(
                      'text-label font-medium leading-none',
                      positive ? 'text-success' : 'text-muted-foreground',
                    )}
                  >
                    {loading ? '…' : delta}
                  </span>
                </div>
              </Card>
            ))}
          </div>
        </section>

        <AnalyticsCard />

        {/* Recent posts */}
        <section>
          <SectionHeader
            action={
              <Link
                href='/posts'
                className='flex items-center gap-0.75 text-label font-semibold text-muted-foreground hover:text-body transition-colors duration-100 active:scale-95 shrink-0'
              >
                View all <ArrowUpRight size={11} strokeWidth={2} />
              </Link>
            }
          >
            Recent Posts
          </SectionHeader>

          <div className='rounded-lg border border-border overflow-hidden bg-card'>
            {recent.length === 0 ? (
              <div className='py-12 text-center'>
                <p className='text-ui text-muted-foreground'>
                  {loading ? 'Loading posts…' : 'No posts yet. Write one, or pull existing posts from the cloud below.'}
                </p>
              </div>
            ) : (
              recent.map((post, i) => (
                <div
                  key={post.id}
                  className={cn(
                    'group flex items-center gap-3 px-4 py-2.75',
                    'hover:bg-muted/50',
                    'transition-colors duration-100',
                    i < recent.length - 1 ? 'border-b border-border/60' : '',
                  )}
                >
                  <StatusDot status={post.status} />

                  <div className='flex-1 min-w-0'>
                    <p className='text-ui font-medium text-foreground truncate leading-none group-hover:text-foreground transition-colors duration-100'>
                      {post.title}
                    </p>
                    <div className='flex items-center gap-2 mt-1.25'>
                      {/* The dot's colour is not something everyone can tell
                          apart, so the status is also written out. */}
                      <span className='text-label text-muted-foreground'>
                        {post.status === 'published' ? 'Published' : 'Draft'}
                      </span>
                      <span className='text-label font-mono text-muted-foreground tracking-tight'>· {post.date}</span>
                      {post.tags.map((t) => (
                        <span key={t} className='text-label text-muted-foreground'>
                          · {t}
                        </span>
                      ))}
                    </div>
                  </div>

                  <ArrowUpRight
                    size={13}
                    strokeWidth={1.8}
                    className='shrink-0 text-muted-foreground group-hover:text-muted-foreground transition-colors duration-100'
                  />
                </div>
              ))
            )}
          </div>
        </section>

        {/* Quick actions */}
        <section>
          <SectionHeader>Quick Actions</SectionHeader>
          <div className='grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-3'>
            <Link href='/posts/new' className={QUICK_CARD_CLASS}>
              <QuickActionInner Icon={Plus} label='New Post' desc='Start writing in Markdown' />
            </Link>
            <Link href='/media' className={QUICK_CARD_CLASS}>
              <QuickActionInner Icon={Upload} label='Upload Media' desc='Add files to Cloudflare R2' />
            </Link>
            <button
              type='button'
              onClick={() => void runSync('pull')}
              disabled={syncing.pull}
              className={QUICK_CARD_CLASS}
            >
              <QuickActionInner
                Icon={CloudDownload}
                label={syncing.pull ? 'Pulling…' : 'Pull from Cloud'}
                desc='Mirror posts from D1 to local'
              />
            </button>
            <button
              type='button'
              onClick={() => void runSync('push')}
              disabled={syncing.push}
              className={QUICK_CARD_CLASS}
            >
              <QuickActionInner
                Icon={CloudUpload}
                label={syncing.push ? 'Pushing…' : 'Push to Cloud'}
                desc='Upload local posts to D1'
              />
            </button>
          </div>
        </section>
      </div>
    </main>
  );
}
