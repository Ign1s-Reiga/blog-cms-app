'use client';

import { useCallback, useEffect, useMemo, useState } from 'react';
import { BarChart2, ExternalLink, KeyRound, Loader2, RefreshCw } from 'lucide-react';
import { useRouter } from 'next/navigation';
import { Button } from '@/components/ui/button';
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { cn } from '@/lib/utils';

/// Mirrors `AnalyticsError` in `src-tauri/src/analytics.rs`.
type TrafficError = {
  kind: 'permission' | 'notConfigured' | 'network' | 'query' | 'local';
  message: string;
};

/// Mirrors `Site` in `src-tauri/src/traffic.rs`.
type Site = { site_tag: string; name: string };

type DailyViews = { date: string; views: number };

/// Mirrors `PostTraffic`.
type PostTraffic = {
  id: number;
  slug: string;
  title: string;
  published: boolean;
  views: number;
  visits: number;
  days: DailyViews[];
};

/// Mirrors `TrafficReport`.
type TrafficReport = {
  dates: string[];
  posts: PostTraffic[];
  unattributed: { path: string; views: number }[];
  total_views: number;
  attributed_views: number;
};

const WINDOWS = [
  { days: 7, label: '7 days' },
  { days: 30, label: '30 days' },
  { days: 90, label: '90 days' },
];

function isTrafficError(e: unknown): e is TrafficError {
  return typeof e === 'object' && e !== null && 'kind' in e && 'message' in e;
}

function shortDate(iso: string): string {
  const [, m, d] = iso.split('-');
  return `${Number(m)}/${Number(d)}`;
}

export default function AnalyticsPage() {
  const router = useRouter();
  const [days, setDays] = useState(7);
  const [report, setReport] = useState<TrafficReport | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<TrafficError | null>(null);

  /// The post whose daily series is drawn. `null` draws the whole blog.
  const [focused, setFocused] = useState<number | null>(null);

  // Only fetched when there is no site configured yet, so the empty state can
  // offer the choice instead of describing it.
  const [sites, setSites] = useState<Site[] | null>(null);
  const [choosing, setChoosing] = useState(false);

  const load = useCallback(async () => {
    const { invoke, isTauri } = await import('@tauri-apps/api/core');
    if (!isTauri()) {
      setLoading(false);
      return;
    }
    setLoading(true);
    try {
      setReport(await invoke<TrafficReport>('fetch_post_traffic', { days }));
      setError(null);
    } catch (e) {
      setError(isTrafficError(e) ? e : { kind: 'query', message: String(e) });
      setReport(null);
    } finally {
      setLoading(false);
    }
  }, [days]);

  useEffect(() => {
    void load();
  }, [load]);

  const offerSites = async () => {
    const { invoke, isTauri } = await import('@tauri-apps/api/core');
    if (!isTauri()) return;
    setChoosing(true);
    try {
      setSites(await invoke<Site[]>('list_web_analytics_sites'));
    } catch (e) {
      setError(isTrafficError(e) ? e : { kind: 'query', message: String(e) });
    } finally {
      setChoosing(false);
    }
  };

  const chooseSite = async (tag: string) => {
    const { invoke, isTauri } = await import('@tauri-apps/api/core');
    if (!isTauri()) return;
    setChoosing(true);
    try {
      // The other three are sent as they stand: `save_settings` writes whatever
      // it is given, and this screen is not where they are edited.
      const creds = await invoke<{
        r2_public_url: string;
        thumbnail_key_pattern: string;
        media_key_pattern: string;
      } | null>('get_credentials');
      if (!creds) return;
      await invoke('save_settings', {
        r2PublicUrl: creds.r2_public_url,
        thumbnailKeyPattern: creds.thumbnail_key_pattern,
        mediaKeyPattern: creds.media_key_pattern,
        webAnalyticsSiteTag: tag,
      });
      setSites(null);
      await load();
    } catch (e) {
      setError(isTrafficError(e) ? e : { kind: 'query', message: String(e) });
    } finally {
      setChoosing(false);
    }
  };

  const series = useMemo(() => {
    if (!report) return [];
    if (focused === null) {
      // The whole blog, posts and everything else alike.
      return report.dates.map((date, i) => ({
        date,
        views: report.posts.reduce((sum, p) => sum + (p.days[i]?.views ?? 0), 0),
      }));
    }
    return report.posts.find((p) => p.id === focused)?.days ?? [];
  }, [report, focused]);

  const peak = Math.max(1, ...series.map((d) => d.views));

  return (
    <main className='flex-1 overflow-y-auto p-6'>
      <div className='max-w-[1000px] space-y-5'>
        <div className='flex items-start justify-between gap-4'>
          <div>
            <h1 className='text-[15px] font-semibold text-zinc-800 dark:text-zinc-200'>Analytics</h1>
            <p className='text-[12px] text-zinc-500 dark:text-zinc-600'>
              What readers opened, from Cloudflare Web Analytics.
            </p>
          </div>
          <div className='flex items-center gap-2'>
            <Tabs value={String(days)} onValueChange={(v) => setDays(Number(v))}>
              <TabsList className='h-[30px] gap-px rounded-[7px] border border-zinc-200 dark:border-white/[0.07] bg-zinc-100 dark:bg-white/[0.04] p-[3px]'>
                {WINDOWS.map((w) => (
                  <TabsTrigger key={w.days} value={String(w.days)} className='h-[22px] px-2.5 text-[11px]'>
                    {w.label}
                  </TabsTrigger>
                ))}
              </TabsList>
            </Tabs>
            <Button
              variant='ghost'
              size='sm'
              onClick={() => (sites === null ? void offerSites() : setSites(null))}
              disabled={choosing}
              className='h-[30px] text-[11px] text-zinc-500 dark:text-zinc-500'
            >
              {choosing ? 'Looking…' : sites === null ? 'Change site' : 'Cancel'}
            </Button>
            <Button
              variant='ghost'
              size='icon'
              aria-label='Refresh'
              onClick={() => void load()}
              disabled={loading}
              className='size-[30px] rounded-[6px] text-zinc-400 dark:text-zinc-500'
            >
              <RefreshCw size={13} strokeWidth={2} className={cn(loading && 'animate-spin')} />
            </Button>
          </div>
        </div>

        {loading && !report ? (
          <p className='flex items-center gap-2 p-3 text-[12px] text-zinc-500 dark:text-zinc-500'>
            <Loader2 size={13} strokeWidth={2} className='animate-spin' />
            Reading traffic…
          </p>
        ) : error ? (
          <ErrorPanel
            error={error}
            sites={sites}
            choosing={choosing}
            onOfferSites={() => void offerSites()}
            onChoose={(tag) => void chooseSite(tag)}
            onSettings={() => router.push('/settings')}
          />
        ) : report ? (
          <>
            {sites !== null && (
              <section className='rounded-[8px] border border-zinc-200 bg-white px-4 py-3 dark:border-white/[0.07] dark:bg-[#161616]'>
                <h2 className='text-[13px] font-semibold text-zinc-800 dark:text-zinc-200'>Which site is the blog?</h2>
                {sites.length === 0 ? (
                  <p className='mt-1.5 text-[12px] text-zinc-500 dark:text-zinc-600'>
                    This account has no Web Analytics sites.
                  </p>
                ) : (
                  <ul className='mt-2 space-y-1'>
                    {sites.map((site) => (
                      <li key={site.site_tag}>
                        <button
                          type='button'
                          onClick={() => void chooseSite(site.site_tag)}
                          disabled={choosing}
                          className='flex w-full items-center justify-between gap-3 rounded-[6px] border border-zinc-200 px-3 py-2 text-left transition-colors hover:bg-zinc-50 active:scale-[0.99] disabled:opacity-60 dark:border-white/[0.07] dark:hover:bg-white/[0.03]'
                        >
                          <span className='truncate text-[12px] text-zinc-700 dark:text-zinc-300'>{site.name}</span>
                          <span className='shrink-0 font-mono text-[10px] text-zinc-400 dark:text-zinc-600'>
                            {site.site_tag.slice(0, 8)}…
                          </span>
                        </button>
                      </li>
                    ))}
                  </ul>
                )}
              </section>
            )}

            <div className='grid gap-4 sm:grid-cols-3'>
              <Stat label='Views' value={report.total_views} hint='Every page on the blog' />
              <Stat label='On posts' value={report.attributed_views} hint='Matched to a post in the library' />
              <Stat
                label='Posts read'
                value={report.posts.length}
                hint={`of ${report.posts.length === 0 ? 'none' : 'those with any traffic'}`}
              />
            </div>

            <section className='rounded-[8px] border border-zinc-200 dark:border-white/[0.07] bg-white dark:bg-[#161616]'>
              <div className='flex items-baseline justify-between gap-3 border-b border-zinc-100 dark:border-white/[0.05] px-4 py-3'>
                <h2 className='text-[13px] font-semibold text-zinc-800 dark:text-zinc-200'>
                  {focused === null ? 'All posts' : report.posts.find((p) => p.id === focused)?.title}
                </h2>
                {focused !== null && (
                  <button
                    type='button'
                    onClick={() => setFocused(null)}
                    className='text-[11px] text-zinc-500 transition-colors hover:text-zinc-800 dark:text-zinc-500 dark:hover:text-zinc-300'
                  >
                    Show all
                  </button>
                )}
              </div>
              <div className='px-4 py-4'>
                {series.length === 0 || peak === 1 ? (
                  <p className='py-6 text-center text-[12px] text-zinc-400 dark:text-zinc-600'>
                    Nothing recorded in this window.
                  </p>
                ) : (
                  <div className='flex h-[140px] items-end gap-[3px]'>
                    {series.map((d) => (
                      <div key={d.date} className='group relative flex flex-1 flex-col justify-end'>
                        <div
                          className='rounded-t-[2px] bg-zinc-300 transition-colors group-hover:bg-zinc-500 dark:bg-zinc-700 dark:group-hover:bg-zinc-500'
                          style={{ height: `${Math.max(2, (d.views / peak) * 100)}%` }}
                        />
                        <span className='pointer-events-none absolute -top-5 left-1/2 -translate-x-1/2 whitespace-nowrap rounded-[4px] bg-zinc-900 px-1.5 py-0.5 text-[10px] text-white opacity-0 transition-opacity group-hover:opacity-100 dark:bg-zinc-100 dark:text-zinc-900'>
                          {shortDate(d.date)} · {d.views}
                        </span>
                      </div>
                    ))}
                  </div>
                )}
              </div>
            </section>

            <section className='rounded-[8px] border border-zinc-200 dark:border-white/[0.07] bg-white dark:bg-[#161616]'>
              <div className='border-b border-zinc-100 dark:border-white/[0.05] px-4 py-3'>
                <h2 className='text-[13px] font-semibold text-zinc-800 dark:text-zinc-200'>By post</h2>
              </div>
              {report.posts.length === 0 ? (
                <p className='px-4 py-6 text-center text-[12px] text-zinc-400 dark:text-zinc-600'>
                  No traffic reached a post in this window.
                </p>
              ) : (
                <ul className='p-2'>
                  {report.posts.map((p) => (
                    <li key={p.id}>
                      <button
                        type='button'
                        onClick={() => setFocused(focused === p.id ? null : p.id)}
                        onDoubleClick={() => router.push(`/posts/edit?id=${p.id}`)}
                        className={cn(
                          'flex w-full items-baseline gap-3 rounded-[6px] px-2.5 py-2 text-left transition-colors',
                          focused === p.id
                            ? 'bg-zinc-100 dark:bg-white/[0.06]'
                            : 'hover:bg-zinc-50 dark:hover:bg-white/[0.03]',
                        )}
                      >
                        <span className='min-w-0 flex-1'>
                          <span className='block truncate text-[12px] font-medium text-zinc-700 dark:text-zinc-300'>
                            {p.title}
                          </span>
                          <span className='block truncate font-mono text-[10px] text-zinc-400 dark:text-zinc-600'>
                            {p.slug}
                            {!p.published && ' · not published'}
                          </span>
                        </span>
                        <span className='shrink-0 text-right'>
                          <span className='block text-[12px] font-semibold tabular-nums text-zinc-800 dark:text-zinc-200'>
                            {p.views.toLocaleString()}
                          </span>
                          <span className='block text-[10px] tabular-nums text-zinc-400 dark:text-zinc-600'>
                            {p.visits.toLocaleString()} visits
                          </span>
                        </span>
                      </button>
                    </li>
                  ))}
                </ul>
              )}
            </section>

            {/* Shown, not swallowed: an index page and a post whose URL does not
                end in its slug look identical in a total, and only one of them
                is worth doing something about. */}
            {report.unattributed.length > 0 && (
              <section className='rounded-[8px] border border-zinc-200 dark:border-white/[0.07] bg-white dark:bg-[#161616]'>
                <div className='border-b border-zinc-100 dark:border-white/[0.05] px-4 py-3'>
                  <h2 className='text-[13px] font-semibold text-zinc-800 dark:text-zinc-200'>Other paths</h2>
                  <p className='mt-0.5 text-[11px] text-zinc-500 dark:text-zinc-600'>
                    Traffic that did not end in a post’s slug — the blog’s own pages, and anything whose URL this could
                    not match.
                  </p>
                </div>
                <ul className='p-2'>
                  {report.unattributed.slice(0, 25).map((u) => (
                    <li key={u.path} className='flex items-baseline gap-3 px-2.5 py-1.5'>
                      <span className='min-w-0 flex-1 truncate font-mono text-[11px] text-zinc-600 dark:text-zinc-400'>
                        {u.path}
                      </span>
                      <span className='shrink-0 text-[11px] tabular-nums text-zinc-500 dark:text-zinc-500'>
                        {u.views.toLocaleString()}
                      </span>
                    </li>
                  ))}
                </ul>
                {report.unattributed.length > 25 && (
                  <p className='px-4 pb-3 text-[11px] text-zinc-400 dark:text-zinc-600'>
                    …and {report.unattributed.length - 25} more.
                  </p>
                )}
              </section>
            )}
          </>
        ) : null}
      </div>
    </main>
  );
}

function Stat({ label, value, hint }: { label: string; value: number; hint: string }) {
  return (
    <div className='rounded-[8px] border border-zinc-200 dark:border-white/[0.07] bg-white px-4 py-3 dark:bg-[#161616]'>
      <p className='text-[11px] text-zinc-500 dark:text-zinc-600'>{label}</p>
      <p className='mt-1 text-[20px] font-semibold tabular-nums text-zinc-800 dark:text-zinc-200'>
        {value.toLocaleString()}
      </p>
      <p className='mt-0.5 text-[11px] text-zinc-400 dark:text-zinc-600'>{hint}</p>
    </div>
  );
}

/// Every way this can fail says which one it is and what to do about it.
///
/// A missing site and a refused token both produce an empty chart, and telling
/// them apart is the difference between one click and an afternoon.
function ErrorPanel({
  error,
  sites,
  choosing,
  onOfferSites,
  onChoose,
  onSettings,
}: {
  error: TrafficError;
  sites: Site[] | null;
  choosing: boolean;
  onOfferSites: () => void;
  onChoose: (tag: string) => void;
  onSettings: () => void;
}) {
  if (error.kind === 'notConfigured' && error.message.includes('Web Analytics')) {
    return (
      <div className='rounded-[8px] border border-dashed border-zinc-200 px-4 py-8 text-center dark:border-white/[0.08]'>
        <BarChart2 size={18} strokeWidth={1.6} className='mx-auto text-zinc-300 dark:text-zinc-700' />
        <p className='mx-auto mt-2 max-w-[440px] text-[12px] leading-[1.6] text-zinc-500 dark:text-zinc-600'>
          Readership comes from Cloudflare Web Analytics. Pick which site is the blog and this reads from it — no new
          token permission is needed.
        </p>

        {sites === null ? (
          <Button size='sm' onClick={onOfferSites} disabled={choosing} className='mt-3 h-[30px] text-[12px]'>
            {choosing ? 'Looking…' : 'Choose a site'}
          </Button>
        ) : sites.length === 0 ? (
          <p className='mt-3 text-[12px] text-zinc-500 dark:text-zinc-600'>
            This account has no Web Analytics sites. Add one in the Cloudflare dashboard, then come back.
          </p>
        ) : (
          <ul className='mx-auto mt-3 max-w-[340px] space-y-1'>
            {sites.map((s) => (
              <li key={s.site_tag}>
                <button
                  type='button'
                  onClick={() => onChoose(s.site_tag)}
                  disabled={choosing}
                  className='flex w-full items-center justify-between gap-3 rounded-[6px] border border-zinc-200 px-3 py-2 text-left transition-colors hover:bg-zinc-50 active:scale-[0.99] disabled:opacity-60 dark:border-white/[0.07] dark:hover:bg-white/[0.03]'
                >
                  <span className='truncate text-[12px] text-zinc-700 dark:text-zinc-300'>{s.name}</span>
                  <span className='shrink-0 font-mono text-[10px] text-zinc-400 dark:text-zinc-600'>
                    {s.site_tag.slice(0, 8)}…
                  </span>
                </button>
              </li>
            ))}
          </ul>
        )}
      </div>
    );
  }

  if (error.kind === 'permission') {
    return (
      <div className='rounded-[8px] border border-amber-200 bg-amber-50/60 px-4 py-4 dark:border-amber-900/40 dark:bg-amber-950/20'>
        <p className='flex items-center gap-2 text-[12px] font-semibold text-amber-700 dark:text-amber-500'>
          <KeyRound size={13} strokeWidth={2} />
          The token cannot read analytics
        </p>
        <p className='mt-1.5 text-[12px] leading-[1.6] text-amber-700/90 dark:text-amber-500/90'>
          Add <span className='font-mono'>Account Analytics: Read</span> to the API token in the Cloudflare dashboard,
          then sign in again. Everything else in the app works without it.
        </p>
        <p className='mt-1.5 text-[11px] text-amber-700/70 dark:text-amber-500/70'>{error.message}</p>
      </div>
    );
  }

  if (error.kind === 'notConfigured') {
    return (
      <Panel title='Not signed in' body={error.message}>
        <Button size='sm' onClick={onSettings} className='mt-3 h-[30px] gap-1.5 text-[12px]'>
          <ExternalLink size={12} strokeWidth={2} />
          Settings
        </Button>
      </Panel>
    );
  }

  return (
    <Panel
      title={
        error.kind === 'network'
          ? 'Could not reach Cloudflare'
          : error.kind === 'local'
            ? 'Could not read the local library'
            : 'Cloudflare did not answer with usable data'
      }
      body={error.message}
    />
  );
}

function Panel({ title, body, children }: { title: string; body: string; children?: React.ReactNode }) {
  return (
    <div className='rounded-[8px] border border-zinc-200 bg-white px-4 py-4 dark:border-white/[0.07] dark:bg-[#161616]'>
      <p className='text-[12px] font-semibold text-zinc-800 dark:text-zinc-200'>{title}</p>
      <p className='mt-1.5 text-[12px] leading-[1.6] text-zinc-600 dark:text-zinc-400'>{body}</p>
      {children}
    </div>
  );
}
