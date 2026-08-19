'use client';

import { useCallback, useEffect, useRef, useState } from 'react';
import { Lock, RefreshCw, TriangleAlert } from 'lucide-react';
import { SectionHeader } from '@/components/SectionHeader';
import { cn } from '@/lib/utils';

// ─── Types (mirror src-tauri/src/analytics.rs) ────────────────────────────────

interface DailyUsage {
  date: string;
  r2_requests: number;
  d1_queries: number;
}

interface Analytics {
  days: DailyUsage[];
  r2_total: number;
  d1_total: number;
}

type ErrorKind = 'permission' | 'notConfigured' | 'network' | 'query';
interface AnalyticsError {
  kind: ErrorKind;
  message: string;
}

// ─── Chart ────────────────────────────────────────────────────────────────────

const W = 224;
const H = 56;
const SLOT = W / 7;
const GAP = 2; // surface gap between adjacent bars
const RADIUS = 4; // rounded data-end, square at the baseline

/// Bar with rounded top corners only — the value end is rounded, the baseline
/// end stays square so bars sit flat on the axis.
function barPath(x: number, y: number, w: number, h: number): string {
  const r = Math.min(RADIUS, h, w / 2);
  return [
    `M${x},${y + h}`,
    `L${x},${y + r}`,
    `Q${x},${y} ${x + r},${y}`,
    `L${x + w - r},${y}`,
    `Q${x + w},${y} ${x + w},${y + r}`,
    `L${x + w},${y + h}`,
    'Z',
  ].join(' ');
}

/// One measure over seven days. Deliberately a small multiple rather than a
/// second series on a shared axis: R2 requests and D1 queries have unrelated
/// magnitudes, and a dual axis would invent a relationship between them.
function Series({
  label,
  total,
  values,
  dates,
  barClass,
}: {
  label: string;
  total: number;
  values: number[];
  dates: string[];
  barClass: string;
}) {
  const [hover, setHover] = useState<number | null>(null);
  // A flat-zero week must not render as full-height bars.
  const peak = Math.max(...values, 1);

  return (
    <figure className='m-0 min-w-0 flex-1'>
      <figcaption className='flex items-baseline justify-between gap-2'>
        <span className='text-[11px] font-semibold text-zinc-600 dark:text-zinc-400'>{label}</span>
        <span className='font-mono text-[11px] tabular-nums text-zinc-500 dark:text-zinc-500'>
          {hover === null ? `${formatCount(total)} total` : `${dates[hover].slice(5)} · ${formatCount(values[hover])}`}
        </span>
      </figcaption>

      <svg
        viewBox={`0 0 ${W} ${H}`}
        className='mt-2 h-14 w-full'
        role='img'
        aria-label={`${label}: ${formatCount(total)} over the last ${values.length} days`}
        onMouseLeave={() => setHover(null)}
      >
        {values.map((v, i) => {
          const h = Math.max(1, Math.round((v / peak) * (H - 2)));
          const x = i * SLOT + GAP / 2;
          const w = SLOT - GAP;
          return (
            <g key={dates[i]} onMouseEnter={() => setHover(i)}>
              {/* Hit target spans the full slot height, not just the bar. */}
              <rect x={i * SLOT} y={0} width={SLOT} height={H} fill='transparent' />
              <path
                d={barPath(x, H - h, w, h)}
                className={cn(barClass, 'transition-opacity duration-100')}
                opacity={hover === null || hover === i ? 1 : 0.45}
              />
              <title>{`${dates[i]}: ${formatCount(v)}`}</title>
            </g>
          );
        })}
      </svg>
    </figure>
  );
}

// ─── Card ─────────────────────────────────────────────────────────────────────

/// Basic R2/D1 usage for the last seven days.
///
/// Re-fetched on every mount, so opening the dashboard always shows current
/// numbers rather than whatever was cached from a previous visit.
export function AnalyticsCard() {
  const [data, setData] = useState<Analytics | null>(null);
  const [error, setError] = useState<AnalyticsError | null>(null);
  const [loading, setLoading] = useState(true);

  /// Which request the card is currently showing the result of.
  ///
  /// Nothing cancels an in-flight query, and "Try again" stays clickable while
  /// one is running — so a request that hangs and fails a minute later would
  /// otherwise land on top of a later one that succeeded, replacing a correct
  /// chart with an error about a request nobody is waiting for any more. Only
  /// the newest attempt is allowed to write.
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
      const fetched = await invoke<Analytics>('fetch_analytics', { days: 7 });
      if (!current()) return;
      setData(fetched);
      setError(null);
    } catch (err) {
      if (!current()) return;
      // The Rust command's error type serialises to { kind, message }; anything
      // else is unexpected and reported as a query failure.
      const e = err as Partial<AnalyticsError>;
      setError(
        e && typeof e.kind === 'string'
          ? { kind: e.kind as ErrorKind, message: e.message ?? '' }
          : { kind: 'query', message: String(err) },
      );
      setData(null);
    } finally {
      // A superseded attempt must not clear the flag either: the request that
      // replaced it is still running, and the overlay belongs to that one.
      if (current()) setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const days = data?.days ?? [];
  // Placeholder shape so the chart keeps its footprint while loading or blocked,
  // and the overlay has something to sit over.
  const dates = days.length ? days.map((d) => d.date) : Array.from({ length: 7 }, (_, i) => `day-${i}`);
  const r2 = days.length ? days.map((d) => d.r2_requests) : Array(7).fill(0);
  const d1 = days.length ? days.map((d) => d.d1_queries) : Array(7).fill(0);

  return (
    <section>
      <SectionHeader>Activity</SectionHeader>

      <div className='relative rounded-lg border border-zinc-200 bg-white p-4 dark:border-white/[0.07] dark:bg-[#161616]'>
        <div className='flex flex-col gap-5 sm:flex-row sm:gap-6'>
          <Series
            label='R2 requests'
            total={data?.r2_total ?? 0}
            values={r2}
            dates={dates}
            barClass='fill-[#2a78d6] dark:fill-[#3987e5]'
          />
          <Series
            label='D1 queries'
            total={data?.d1_total ?? 0}
            values={d1}
            dates={dates}
            barClass='fill-[#eb6834] dark:fill-[#d95926]'
          />
        </div>

        <p className='mt-3 text-[10px] text-zinc-400 dark:text-zinc-600'>Last 7 days</p>

        {/* Values in text, so the chart is never the only way to read them. */}
        {data && (
          <table className='sr-only'>
            <caption>R2 requests and D1 queries per day, last 7 days</caption>
            <thead>
              <tr>
                <th scope='col'>Date</th>
                <th scope='col'>R2 requests</th>
                <th scope='col'>D1 queries</th>
              </tr>
            </thead>
            <tbody>
              {days.map((d) => (
                <tr key={d.date}>
                  <th scope='row'>{d.date}</th>
                  <td>{d.r2_requests}</td>
                  <td>{d.d1_queries}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}

        {loading && !error && (
          <Overlay>
            <RefreshCw size={13} strokeWidth={2} className='animate-spin text-zinc-400' />
            <span className='text-[12px] text-zinc-500 dark:text-zinc-400'>Loading analytics…</span>
          </Overlay>
        )}

        {error && <ErrorOverlay error={error} onRetry={() => void load()} />}
      </div>
    </section>
  );
}

/// Sits over the chart rather than replacing it, so the card keeps its size and
/// the reason is attached to what it affects.
function Overlay({ children }: { children: React.ReactNode }) {
  return (
    <div className='absolute inset-0 flex flex-col items-center justify-center gap-1.5 rounded-lg bg-white/80 px-4 text-center backdrop-blur-[1px] dark:bg-[#161616]/80'>
      {children}
    </div>
  );
}

function ErrorOverlay({ error, onRetry }: { error: AnalyticsError; onRetry: () => void }) {
  const permission = error.kind === 'permission';
  return (
    <Overlay>
      <div className='flex items-center gap-1.5'>
        {permission ? (
          <Lock size={13} strokeWidth={2} className='text-amber-600 dark:text-amber-500' />
        ) : (
          <TriangleAlert size={13} strokeWidth={2} className='text-red-600 dark:text-red-400' />
        )}
        <span
          className={cn(
            'text-[12px] font-semibold',
            permission ? 'text-amber-700 dark:text-amber-500' : 'text-red-600 dark:text-red-400',
          )}
        >
          {permission ? 'Insufficient permissions' : TITLES[error.kind]}
        </span>
      </div>
      <p className='max-w-[380px] text-[11px] leading-[1.5] text-zinc-500 dark:text-zinc-500'>
        {permission
          ? 'The API token needs Account Analytics: Read to show usage.'
          : error.message || 'Analytics could not be loaded.'}
      </p>
      {!permission && (
        <button
          onClick={onRetry}
          className='mt-0.5 text-[11px] font-semibold text-zinc-600 underline underline-offset-2 transition-colors hover:text-zinc-900 dark:text-zinc-400 dark:hover:text-zinc-200'
        >
          Try again
        </button>
      )}
    </Overlay>
  );
}

const TITLES: Record<ErrorKind, string> = {
  permission: 'Insufficient permissions',
  notConfigured: 'Not connected',
  network: 'Cloudflare unreachable',
  query: 'Analytics unavailable',
};

function formatCount(n: number): string {
  if (n < 1000) return String(n);
  if (n < 1_000_000) return `${(n / 1000).toFixed(n < 10_000 ? 1 : 0)}k`;
  return `${(n / 1_000_000).toFixed(1)}M`;
}
