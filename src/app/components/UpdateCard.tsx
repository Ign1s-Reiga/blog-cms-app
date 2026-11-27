'use client';

import { useCallback, useEffect, useState } from 'react';
import { ArrowUpCircle, CheckCircle2, Download, RefreshCw, RotateCcw, TriangleAlert } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { cn } from '@/lib/utils';
import { checkForUpdate, getCachedStatus, installUpdate, restartApp, type UpdateStatus } from '@/lib/updater';
import { isTauri } from '@tauri-apps/api/core';
import { getVersion } from '@tauri-apps/api/app';

type Phase = 'idle' | 'checking' | 'downloading' | 'ready' | 'error';

/// Settings panel for in-app upgrades: shows the running version, checks GitHub
/// Releases for a newer one, and drives download → install → restart.
export function UpdateCard() {
  const [status, setStatus] = useState<UpdateStatus | null>(getCachedStatus);
  const [phase, setPhase] = useState<Phase>('idle');
  const [error, setError] = useState<string | null>(null);
  const [progress, setProgress] = useState<{ downloaded: number; total: number | null } | null>(null);
  const [version, setVersion] = useState<string | null>(status?.current_version ?? null);

  // The running version, so the card reads correctly before any check lands.
  useEffect(() => {
    let active = true;
    void (async () => {
      if (!isTauri()) return;
      const v = await getVersion();
      if (active) setVersion((prev) => prev ?? v);
    })();
    return () => {
      active = false;
    };
  }, []);

  const check = useCallback(async (force: boolean) => {
    setPhase('checking');
    setError(null);
    try {
      const next = await checkForUpdate(force);
      if (next) {
        setStatus(next);
        setVersion(next.current_version);
      }
      setPhase('idle');
    } catch (err) {
      setError(String(err));
      setPhase('error');
    }
  }, []);

  // Replay the launch-time check (or run one if this is the first mount).
  useEffect(() => {
    void check(false);
  }, [check]);

  const install = useCallback(async () => {
    setPhase('downloading');
    setError(null);
    setProgress({ downloaded: 0, total: null });
    try {
      await installUpdate(setProgress);
      // Reached only where the installer does not replace this process
      // (on Windows the app is closed by the installer instead).
      setPhase('ready');
    } catch (err) {
      setError(String(err));
      setPhase('error');
    }
  }, []);

  const busy = phase === 'checking' || phase === 'downloading';
  const available = status?.available === true;

  return (
    <section className='rounded-[8px] border border-border bg-card'>
      <div className='flex items-center justify-between gap-3 px-4 py-3 border-b border-border/60'>
        <h2 className='text-heading font-semibold text-foreground'>Software update</h2>
        <span className='font-mono text-label text-muted-foreground tabular-nums'>v{version ?? '—'}</span>
      </div>

      <div className='px-4 py-3 space-y-3 text-meta'>
        {/* What the visible line below says, at the grain of a phase. The line
            itself is not live because while downloading it rewrites its byte
            count on every chunk, and a screen reader would read out each one;
            the exact figure is on the progress bar for anyone who asks. */}
        <p role='status' className='sr-only'>
          {phase === 'error'
            ? `Update failed: ${error ?? 'the check did not complete.'}`
            : phase === 'checking'
              ? 'Checking for updates…'
              : phase === 'downloading'
                ? 'Downloading update…'
                : phase === 'ready'
                  ? 'Update installed. Restart to finish.'
                  : available
                    ? `Version ${status?.version} is available.`
                    : status
                      ? "You're running the latest version."
                      : ''}
        </p>

        {phase === 'error' ? (
          <StatusLine icon={TriangleAlert} tone='danger' text={error ?? 'Update check failed.'} />
        ) : phase === 'checking' ? (
          <StatusLine icon={RefreshCw} tone='muted' text='Checking GitHub for a newer release…' spin />
        ) : phase === 'downloading' ? (
          <StatusLine icon={Download} tone='muted' text={downloadLabel(progress)} />
        ) : phase === 'ready' ? (
          <StatusLine icon={CheckCircle2} tone='ok' text='Update installed. Restart to finish.' />
        ) : available ? (
          <StatusLine
            icon={ArrowUpCircle}
            tone='accent'
            text={`Version ${status?.version} is available${status?.date ? ` — released ${status.date}` : ''}.`}
          />
        ) : status ? (
          <StatusLine icon={CheckCircle2} tone='ok' text="You're running the latest version." />
        ) : (
          <StatusLine icon={RefreshCw} tone='muted' text='Update checks are unavailable in the browser.' />
        )}

        {phase === 'downloading' && <ProgressBar progress={progress} />}

        {available && phase !== 'downloading' && phase !== 'ready' && status?.notes && (
          <div className='rounded-[6px] border border-border/60 bg-muted/50 px-3 py-2'>
            <p className='text-label font-bold uppercase tracking-[0.12em] text-muted-foreground mb-1'>Release notes</p>
            <pre className='max-h-[160px] overflow-y-auto whitespace-pre-wrap break-words font-sans text-meta leading-[1.55] text-body'>
              {status.notes.trim()}
            </pre>
          </div>
        )}

        <div className='flex items-center gap-2 pt-0.5'>
          {phase === 'ready' ? (
            <Button size='sm' onClick={() => void restartApp()} className='h-[30px] gap-1.5 text-meta font-semibold'>
              <RotateCcw size={13} strokeWidth={2} />
              Restart now
            </Button>
          ) : available ? (
            <Button
              size='sm'
              onClick={() => void install()}
              disabled={busy}
              className='h-[30px] gap-1.5 text-meta font-semibold'
            >
              <Download size={13} strokeWidth={2} className={cn(phase === 'downloading' && 'animate-pulse')} />
              {phase === 'downloading' ? 'Downloading…' : `Update to ${status?.version}`}
            </Button>
          ) : null}

          <Button
            variant='outline'
            size='sm'
            onClick={() => void check(true)}
            disabled={busy}
            className='h-[30px] gap-1.5 text-meta font-semibold'
          >
            <RefreshCw size={13} strokeWidth={2} className={cn(phase === 'checking' && 'animate-spin')} />
            Check for updates
          </Button>
        </div>
      </div>
    </section>
  );
}

// ─── Pieces ───────────────────────────────────────────────────────────────────

const TONES = {
  muted: 'text-muted-foreground',
  ok: 'text-success',
  accent: 'text-body',
  danger: 'text-destructive',
} as const;

function StatusLine({
  icon: Icon,
  tone,
  text,
  spin,
}: {
  icon: React.ComponentType<{ size?: number; strokeWidth?: number; className?: string }>;
  tone: keyof typeof TONES;
  text: string;
  spin?: boolean;
}) {
  return (
    <div className={cn('flex items-start gap-2', TONES[tone])}>
      <Icon size={13} strokeWidth={2} className={cn('mt-[2px] shrink-0', spin && 'animate-spin')} />
      <span className='leading-[1.5]'>{text}</span>
    </div>
  );
}

function ProgressBar({ progress }: { progress: { downloaded: number; total: number | null } | null }) {
  const pct = progress?.total ? Math.min(100, (progress.downloaded / progress.total) * 100) : null;
  return (
    <div
      role='progressbar'
      aria-label='Update download'
      aria-valuemin={0}
      aria-valuemax={100}
      // Left out without a content length: an absent value is how a progress
      // bar says it is indeterminate, rather than claiming zero.
      aria-valuenow={pct === null ? undefined : Math.round(pct)}
      aria-valuetext={downloadLabel(progress)}
      className='h-[4px] w-full overflow-hidden rounded-full bg-muted'
    >
      <div
        // Without a content length there is nothing to measure against, so the
        // bar falls back to an indeterminate pulse.
        className={cn(
          'h-full rounded-full bg-primary',
          pct === null ? 'w-1/3 animate-pulse' : 'transition-[width] duration-200 ease-out',
        )}
        style={pct === null ? undefined : { width: `${pct}%` }}
      />
    </div>
  );
}

function downloadLabel(progress: { downloaded: number; total: number | null } | null): string {
  if (!progress) return 'Starting download…';
  const done = formatBytes(progress.downloaded);
  return progress.total
    ? `Downloading update — ${done} of ${formatBytes(progress.total)}`
    : `Downloading update — ${done}`;
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const mb = bytes / (1024 * 1024);
  return mb < 1 ? `${(bytes / 1024).toFixed(0)} KB` : `${mb.toFixed(1)} MB`;
}
