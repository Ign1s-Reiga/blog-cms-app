'use client';

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { ArrowUpRight, BarChart2, ExternalLink, KeyRound, Loader2, RefreshCw } from 'lucide-react';
import { useRouter } from 'next/navigation';
import { Button } from '@/components/ui/button';
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group';
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
  /// In the trash here. Its traffic is real, but the editor refuses to open it.
  trashed: boolean;
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
  /// A day so busy it filled the API's group limit on its own, so these numbers
  /// are a floor. Said rather than rounded up to a fact.
  truncated: boolean;
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

  /// Which attempt is the current one. Changing the window or pressing Refresh
  /// starts another request without cancelling the one in flight, and a slow
  /// 7-day answer landing after a quick 30-day one would leave the tab saying
  /// 30 days over the wrong report. Only the newest attempt is allowed to
  /// write — the same guard `AnalyticsCard` uses.
  const attempt = useRef(0);

  const load = useCallback(async () => {
    const { invoke, isTauri } = await import('@tauri-apps/api/core');
    if (!isTauri()) {
      setLoading(false);
      return;
    }
    const mine = ++attempt.current;
    const current = () => mine === attempt.current;
    setLoading(true);
    try {
      const fetched = await invoke<TrafficReport>('fetch_post_traffic', { days });
      if (!current()) return;
      setReport(fetched);
      setError(null);
    } catch (e) {
      if (!current()) return;
      setError(isTrafficError(e) ? e : { kind: 'query', message: String(e) });
      setReport(null);
    } finally {
      if (current()) setLoading(false);
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
  // Not `peak === 1`: a series whose busiest day saw a single view is a real
  // series with a real bar to draw, and calling it "nothing recorded" tells the
  // author their post went unread when it did not.
  const recorded = series.reduce((sum, d) => sum + d.views, 0);
  const focusedTitle = focused === null ? null : (report?.posts.find((p) => p.id === focused)?.title ?? null);

  return (
    <main className='flex-1 overflow-y-auto p-6'>
      <div className='max-w-[1000px] space-y-5'>
        <div className='flex items-start justify-between gap-4'>
          <div>
            <h1 className='text-title font-semibold tracking-tight text-balance text-foreground'>Analytics</h1>
            <p className='text-meta text-muted-foreground'>What readers opened, from Cloudflare Web Analytics.</p>
          </div>
          <div className='flex items-center gap-2'>
            {/* A choice of period, not a set of panels: a single-select toggle
                group, so it reads as one setting rather than tabs with no
                tab panels behind them. */}
            <ToggleGroup
              type='single'
              value={String(days)}
              // Single mode reports '' when the active item is pressed again;
              // a period is always chosen, so that press changes nothing.
              onValueChange={(v) => v && setDays(Number(v))}
              aria-label='Period'
              spacing={0}
              className='h-[30px] gap-px rounded-[7px] border border-border bg-muted p-[3px]'
            >
              {WINDOWS.map((w) => (
                <ToggleGroupItem
                  key={w.days}
                  value={String(w.days)}
                  size='sm'
                  className='h-[22px] min-w-0 rounded-[5px] px-2.5 text-label font-medium text-muted-foreground hover:bg-transparent hover:text-foreground data-[state=on]:bg-card data-[state=on]:text-foreground data-[state=on]:shadow-[0_1px_2px_rgba(0,0,0,0.08)]'
                >
                  {w.label}
                </ToggleGroupItem>
              ))}
            </ToggleGroup>
            <Button
              variant='ghost'
              size='sm'
              onClick={() => (sites === null ? void offerSites() : setSites(null))}
              disabled={choosing}
              className='h-[30px] text-label text-muted-foreground'
            >
              {choosing ? 'Looking…' : sites === null ? 'Change site' : 'Cancel'}
            </Button>
            <Button
              variant='ghost'
              size='icon'
              aria-label='Refresh'
              onClick={() => void load()}
              disabled={loading}
              className='size-[30px] rounded-[6px] text-muted-foreground'
            >
              <RefreshCw size={13} strokeWidth={2} className={cn(loading && 'animate-spin')} />
            </Button>
          </div>
        </div>

        {loading && !report ? (
          <p className='flex items-center gap-2 p-3 text-meta text-muted-foreground'>
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
              <section className='rounded-[8px] border border-border bg-card px-4 py-3'>
                <h2 className='text-heading font-semibold text-foreground'>Which site is the blog?</h2>
                {sites.length === 0 ? (
                  <p className='mt-1.5 text-meta text-muted-foreground'>This account has no Web Analytics sites.</p>
                ) : (
                  <ul className='mt-2 space-y-1'>
                    {sites.map((site) => (
                      <li key={site.site_tag}>
                        <button
                          type='button'
                          onClick={() => void chooseSite(site.site_tag)}
                          disabled={choosing}
                          className='flex w-full items-center justify-between gap-3 rounded-[6px] border border-border px-3 py-2 text-left transition-colors hover:bg-muted/50 active:scale-[0.99] disabled:opacity-60'
                        >
                          <span className='truncate text-meta text-body'>{site.name}</span>
                          <span className='shrink-0 font-mono text-label text-muted-foreground'>
                            {site.site_tag.slice(0, 8)}…
                          </span>
                        </button>
                      </li>
                    ))}
                  </ul>
                )}
              </section>
            )}

            {report.truncated && (
              <p className='rounded-[8px] border border-warning/30 bg-warning/10 px-4 py-2.5 text-label leading-[1.6] text-warning'>
                One day in this window had more distinct pages than Cloudflare will return at once, so these numbers are
                a floor rather than a total. A shorter window will be exact.
              </p>
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

            <section className='rounded-[8px] border border-border bg-card'>
              <div className='flex items-baseline justify-between gap-3 border-b border-border/60 px-4 py-3'>
                <h2 className='text-heading font-semibold text-foreground'>{focusedTitle ?? 'All posts'}</h2>
                {focused !== null && (
                  <button
                    type='button'
                    onClick={() => setFocused(null)}
                    className='text-label text-muted-foreground transition-colors hover:text-foreground'
                  >
                    Show all
                  </button>
                )}
              </div>
              <div className='px-4 py-4'>
                {recorded === 0 ? (
                  <p className='py-6 text-center text-meta text-muted-foreground'>Nothing recorded in this window.</p>
                ) : (
                  <div
                    role='img'
                    aria-label={`Views per day, ${focusedTitle ?? 'all posts'}: ${recorded.toLocaleString()} over the last ${series.length} days`}
                    className='flex h-[140px] items-end gap-[3px]'
                  >
                    {series.map((d) => (
                      <div key={d.date} className='group relative flex flex-1 flex-col justify-end'>
                        <div
                          className='rounded-t-[2px] bg-input transition-colors group-hover:bg-muted-foreground'
                          style={{ height: `${Math.max(2, (d.views / peak) * 100)}%` }}
                        />
                        <span className='pointer-events-none absolute -top-5 left-1/2 -translate-x-1/2 whitespace-nowrap rounded-[4px] bg-primary px-1.5 py-0.5 text-label text-primary-foreground opacity-0 transition-opacity group-hover:opacity-100'>
                          {shortDate(d.date)} · {d.views}
                        </span>
                      </div>
                    ))}
                  </div>
                )}

                {/* Values in text, so the chart is never the only way to read
                    them — the hover labels above reach neither a keyboard nor
                    a screen reader. */}
                {recorded > 0 && (
                  <table className='sr-only'>
                    <caption>Views per day, {focusedTitle ?? 'all posts'}</caption>
                    <thead>
                      <tr>
                        <th scope='col'>Date</th>
                        <th scope='col'>Views</th>
                      </tr>
                    </thead>
                    <tbody>
                      {series.map((d) => (
                        <tr key={d.date}>
                          <th scope='row'>{d.date}</th>
                          <td>{d.views}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                )}
              </div>
            </section>

            <section className='rounded-[8px] border border-border bg-card'>
              <div className='border-b border-border/60 px-4 py-3'>
                <h2 className='text-heading font-semibold text-foreground'>By post</h2>
              </div>
              {report.posts.length === 0 ? (
                <p className='px-4 py-6 text-center text-meta text-muted-foreground'>
                  No traffic reached a post in this window.
                </p>
              ) : (
                <ul className='p-2'>
                  {report.posts.map((p) => (
                    <li key={p.id} className='group flex items-center gap-1'>
                      {/* A toggle: pressing it draws this post's days in the
                          chart above, and pressing it again goes back to all
                          posts — so `aria-pressed`, not colour alone, is what
                          says which one is drawn. */}
                      <button
                        type='button'
                        aria-pressed={focused === p.id}
                        onClick={() => setFocused(focused === p.id ? null : p.id)}
                        // Only for a post the editor will actually open. It
                        // refuses a trashed one, so offering it here would send
                        // every double-click to an error message.
                        onDoubleClick={p.trashed ? undefined : () => router.push(`/posts/edit?id=${p.id}`)}
                        title={p.trashed ? 'In the trash. Restore it from Posts → Trash to open it.' : undefined}
                        className={cn(
                          'flex min-w-0 flex-1 items-baseline gap-3 rounded-[6px] px-2.5 py-2 text-left transition-colors',
                          focused === p.id ? 'bg-muted' : 'hover:bg-muted/50',
                        )}
                      >
                        <span className='min-w-0 flex-1'>
                          <span className='block truncate text-meta font-medium text-body'>{p.title}</span>
                          <span className='block truncate font-mono text-label text-muted-foreground'>
                            {p.slug}
                            {!p.published && ' · not published'}
                            {p.trashed && ' · in the trash'}
                          </span>
                        </span>
                        <span className='shrink-0 text-right'>
                          <span className='block text-meta font-semibold tabular-nums text-foreground'>
                            {p.views.toLocaleString()}
                          </span>
                          <span className='block text-label tabular-nums text-muted-foreground'>
                            {p.visits.toLocaleString()} visits
                          </span>
                        </span>
                      </button>
                      {/* The double-click above has no keyboard equivalent, and
                          Enter on the row already means "draw this one", so
                          opening gets its own control. Same rule about the
                          trash as the double-click. */}
                      {!p.trashed && (
                        <Button
                          variant='ghost'
                          size='icon-sm'
                          aria-label={`Open ${p.title} in the editor`}
                          title='Open in the editor'
                          onClick={() => router.push(`/posts/edit?id=${p.id}`)}
                          className={cn(
                            'shrink-0 text-muted-foreground transition-opacity group-hover:opacity-100 group-focus-within:opacity-100 focus-visible:opacity-100',
                            focused === p.id ? 'opacity-100' : 'opacity-0',
                          )}
                        >
                          <ArrowUpRight size={13} strokeWidth={2} />
                        </Button>
                      )}
                    </li>
                  ))}
                </ul>
              )}
            </section>

            {/* Shown, not swallowed: an index page and a post whose URL does not
                end in its slug look identical in a total, and only one of them
                is worth doing something about. */}
            {report.unattributed.length > 0 && (
              <section className='rounded-[8px] border border-border bg-card'>
                <div className='border-b border-border/60 px-4 py-3'>
                  <h2 className='text-heading font-semibold text-foreground'>Other paths</h2>
                  <p className='mt-0.5 text-label text-muted-foreground'>
                    Traffic that did not end in a post’s slug — the blog’s own pages, and anything whose URL this could
                    not match.
                  </p>
                </div>
                <ul className='p-2'>
                  {report.unattributed.slice(0, 25).map((u) => (
                    <li key={u.path} className='flex items-baseline gap-3 px-2.5 py-1.5'>
                      <span className='min-w-0 flex-1 truncate font-mono text-label text-body'>{u.path}</span>
                      <span className='shrink-0 text-label tabular-nums text-muted-foreground'>
                        {u.views.toLocaleString()}
                      </span>
                    </li>
                  ))}
                </ul>
                {report.unattributed.length > 25 && (
                  <p className='px-4 pb-3 text-label text-muted-foreground'>
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
    <div className='rounded-[8px] border border-border bg-card px-4 py-3'>
      <p className='text-label text-muted-foreground'>{label}</p>
      <p className='mt-1 text-[20px] font-semibold tabular-nums text-foreground'>{value.toLocaleString()}</p>
      <p className='mt-0.5 text-label text-muted-foreground'>{hint}</p>
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
      <div className='rounded-[8px] border border-dashed border-border px-4 py-8 text-center'>
        <BarChart2 size={18} strokeWidth={1.6} className='mx-auto text-muted-foreground' />
        <p className='mx-auto mt-2 max-w-[440px] text-meta leading-[1.6] text-muted-foreground'>
          Readership comes from Cloudflare Web Analytics. Pick which site is the blog and this reads from it — no new
          token permission is needed.
        </p>

        {sites === null ? (
          <Button size='sm' onClick={onOfferSites} disabled={choosing} className='mt-3 h-[30px] text-meta'>
            {choosing ? 'Looking…' : 'Choose a site'}
          </Button>
        ) : sites.length === 0 ? (
          <p className='mt-3 text-meta text-muted-foreground'>
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
                  className='flex w-full items-center justify-between gap-3 rounded-[6px] border border-border px-3 py-2 text-left transition-colors hover:bg-muted/50 active:scale-[0.99] disabled:opacity-60'
                >
                  <span className='truncate text-meta text-body'>{s.name}</span>
                  <span className='shrink-0 font-mono text-label text-muted-foreground'>{s.site_tag.slice(0, 8)}…</span>
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
      <div className='rounded-[8px] border border-warning/30 bg-warning/10 px-4 py-4'>
        <p className='flex items-center gap-2 text-meta font-semibold text-warning'>
          <KeyRound size={13} strokeWidth={2} />
          The token cannot read analytics
        </p>
        <p className='mt-1.5 text-meta leading-[1.6] text-warning'>
          Add <span className='font-mono'>Account Analytics: Read</span> to the API token in the Cloudflare dashboard,
          then sign in again. Everything else in the app works without it.
        </p>
        <p className='mt-1.5 text-label text-warning'>{error.message}</p>
      </div>
    );
  }

  if (error.kind === 'notConfigured') {
    return (
      <Panel title='Not signed in' body={error.message}>
        <Button size='sm' onClick={onSettings} className='mt-3 h-[30px] gap-1.5 text-meta'>
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
    <div className='rounded-[8px] border border-border bg-card px-4 py-4'>
      <p className='text-meta font-semibold text-foreground'>{title}</p>
      <p className='mt-1.5 text-meta leading-[1.6] text-body'>{body}</p>
      {children}
    </div>
  );
}
