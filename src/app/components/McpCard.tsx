'use client';

import { useCallback, useEffect, useId, useState } from 'react';
import { Check, Copy, Eye, EyeOff, KeyRound, Plug, ShieldCheck, TriangleAlert, X } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { cn } from '@/lib/utils';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

type McpStatus = {
  enabled: boolean;
  running: boolean;
  port: number;
  endpoint: string;
  /// Null until the server has been started for the first time — nothing is
  /// issued just for opening this screen.
  token: string | null;
};

/// Mirrors `PublishState` in `src-tauri/src/mcp/publish.rs`. `publishing` is the
/// in-flight state an approved request holds while it uploads — it is neither
/// waiting on this screen nor finished.
type PublishState = 'awaiting_approval' | 'publishing' | 'rejected' | 'published' | 'failed';

type PublishRequest = {
  id: string;
  post_id: number;
  slug: string;
  title: string;
  reason: string | null;
  requested_at: number;
  state: PublishState;
  error: string | null;
};

/// Fallback when the port box is empty or unparseable — mirrors `DEFAULT_PORT`
/// in `src-tauri/src/mcp/mod.rs`.
const DEFAULT_PORT = 4127;

/// Settings panel for the local MCP endpoint: turn it on, hand a client its
/// address and token, and approve or reject the publishes agents ask for.
///
/// The approval list is the whole point of the gate — an MCP client can draft
/// freely but cannot put anything on the blog without a click here.
export function McpCard() {
  const [status, setStatus] = useState<McpStatus | null>(null);
  const [requests, setRequests] = useState<PublishRequest[]>([]);
  const [port, setPort] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [reveal, setReveal] = useState(false);
  const [available, setAvailable] = useState(true);
  const portId = useId();
  const portRangeId = `${portId}-range`;

  const refresh = useCallback(async () => {
    if (!isTauri()) {
      setAvailable(false);
      return;
    }
    try {
      const [next, queue] = await Promise.all([
        invoke<McpStatus>('mcp_status'),
        invoke<PublishRequest[]>('mcp_list_publish_requests'),
      ]);
      setStatus(next);
      setRequests(queue);
      // Only seed the input while it is untouched, so a refresh cannot
      // overwrite a port being typed.
      setPort((prev) => (prev === '' ? String(next.port) : prev));
      setError(null);
    } catch (err) {
      setError(String(err));
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // The queue also moves when an agent asks for something, which happens with
  // no interaction here at all.
  useEffect(() => {
    let stop: (() => void) | undefined;
    let cancelled = false;
    void (async () => {
      if (!isTauri()) return;
      const un = await listen<PublishRequest[]>('mcp://publish-requests-changed', (e) => {
        setRequests(e.payload);
      });
      if (cancelled) un();
      else stop = un;
    })();
    return () => {
      cancelled = true;
      stop?.();
    };
  }, []);

  const run = useCallback(
    async (fn: (invoke: <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>) => Promise<void>) => {
      if (!isTauri() || busy) return;
      setBusy(true);
      setError(null);
      try {
        await fn(invoke);
      } catch (err) {
        setError(String(err));
      } finally {
        setBusy(false);
      }
    },
    [busy],
  );

  const configure = (enabled: boolean, nextPort: number) =>
    run(async (invoke) => {
      const next = await invoke<McpStatus>('mcp_configure', { enabled, port: nextPort });
      setStatus(next);
      setPort(String(next.port));
    });

  const regenerate = () =>
    run(async (invoke) => {
      setStatus(await invoke<McpStatus>('mcp_regenerate_token'));
    });

  const decide = (id: string, approve: boolean) =>
    run(async (invoke) => {
      await invoke<PublishRequest>(approve ? 'mcp_approve_publish' : 'mcp_reject_publish', {
        requestId: id,
      });
      setRequests(await invoke<PublishRequest[]>('mcp_list_publish_requests'));
    });

  const parsedPort = Number.parseInt(port, 10);
  const portValid = Number.isInteger(parsedPort) && parsedPort >= 1024 && parsedPort <= 65535;
  const portDirty = status !== null && portValid && parsedPort !== status.port;

  // Only an unapproved request gets buttons. Everything else has been decided —
  // including one still publishing, which is past the point of being stoppable.
  const pending = requests.filter((r) => r.state === 'awaiting_approval');
  const decided = requests.filter((r) => r.state !== 'awaiting_approval');

  return (
    <section className='rounded-[8px] border border-border bg-card'>
      <div className='flex items-center justify-between gap-3 px-4 py-3 border-b border-border/60'>
        <div>
          <h2 className='text-heading font-semibold text-foreground'>MCP server</h2>
          <p className='mt-0.5 text-label text-muted-foreground'>
            Let an AI assistant read and draft your posts. Publishing still needs your approval.
          </p>
        </div>
        <span
          className={cn(
            'flex shrink-0 items-center gap-1.5 rounded-full px-2 py-[3px] text-label font-bold uppercase tracking-[0.08em]',
            status?.running ? 'bg-success/10 text-success' : 'bg-muted text-muted-foreground',
          )}
        >
          <span className={cn('size-[5px] rounded-full', status?.running ? 'bg-success' : 'bg-muted-foreground')} />
          {status?.running ? 'Running' : 'Stopped'}
        </span>
      </div>

      <div className='px-4 py-3 space-y-4 text-meta'>
        {!available ? (
          <p className='text-muted-foreground'>The MCP server is unavailable in the browser.</p>
        ) : (
          <>
            {error && (
              <p role='alert' className='flex items-start gap-1.5 text-label leading-[1.5] text-destructive'>
                <TriangleAlert size={12} strokeWidth={2} className='mt-[2px] shrink-0' />
                {error}
              </p>
            )}

            <div className='flex items-center gap-2'>
              <Button
                size='sm'
                variant={status?.enabled ? 'outline' : 'default'}
                onClick={() => void configure(!status?.enabled, portValid ? parsedPort : DEFAULT_PORT)}
                disabled={busy || !portValid}
                className='h-[30px] gap-1.5 text-meta font-semibold'
              >
                <Plug size={13} strokeWidth={2} />
                {status?.enabled ? 'Turn off' : 'Turn on'}
              </Button>

              <div className='flex items-center gap-1.5'>
                <Label htmlFor={portId} className='text-meta leading-normal font-normal text-muted-foreground'>
                  Port
                </Label>
                <Input
                  id={portId}
                  aria-invalid={port !== '' && !portValid ? true : undefined}
                  aria-describedby={port !== '' && !portValid ? portRangeId : undefined}
                  value={port}
                  onChange={(e) => setPort(e.target.value)}
                  spellCheck={false}
                  inputMode='numeric'
                  className='h-[30px] w-[84px] font-mono text-meta'
                />
              </div>

              {portDirty && (
                <Button
                  size='sm'
                  variant='outline'
                  onClick={() => void configure(status?.enabled ?? false, parsedPort)}
                  disabled={busy}
                  className='h-[30px] text-meta font-semibold'
                >
                  Apply
                </Button>
              )}
              {port !== '' && !portValid && (
                <span id={portRangeId} className='text-label text-destructive'>
                  1024–65535
                </span>
              )}
            </div>

            {status && (
              <div className='space-y-2 rounded-[6px] border border-border/60 bg-muted/50 px-3 py-2.5'>
                <Secret label='Endpoint' value={status.endpoint} />
                {status.token ? (
                  <Secret label='Token' value={status.token} masked={!reveal} onToggle={() => setReveal((v) => !v)} />
                ) : (
                  <div className='flex items-center justify-between gap-3'>
                    <span className='shrink-0 text-muted-foreground'>Token</span>
                    <span className='truncate text-label text-muted-foreground'>
                      Issued when you first turn the server on
                    </span>
                  </div>
                )}
                {status.token && (
                  <div className='flex flex-wrap items-center gap-2 pt-1'>
                    <CopyButton label='Copy client config' value={clientConfig(status.endpoint, status.token)} />
                    <Button
                      variant='outline'
                      size='sm'
                      onClick={() => void regenerate()}
                      disabled={busy}
                      className='h-[28px] gap-1.5 text-label font-semibold'
                    >
                      <KeyRound size={12} strokeWidth={2} />
                      New token
                    </Button>
                  </div>
                )}
                <p className='text-label leading-[1.5] text-muted-foreground'>
                  Paste the config into your MCP client. The endpoint listens on this machine only, and rejects any
                  request without the token.
                </p>
              </div>
            )}

            <div className='space-y-2 border-t border-border/60 pt-3'>
              <div className='flex items-center gap-1.5'>
                <ShieldCheck size={13} strokeWidth={2} className='text-muted-foreground' />
                <h3 className='text-meta font-semibold text-body'>Publish approvals</h3>
                {pending.length > 0 && (
                  <span className='rounded-full bg-warning/15 px-1.5 py-[1px] text-label font-bold text-warning'>
                    {pending.length}
                  </span>
                )}
              </div>

              {requests.length === 0 ? (
                <p className='text-label leading-[1.5] text-muted-foreground'>
                  Nothing waiting. When an assistant asks to publish a post, it appears here for you to approve.
                </p>
              ) : (
                <ul className='space-y-1.5'>
                  {pending.map((r) => (
                    <li key={r.id} className='rounded-[6px] border border-warning/30 bg-warning/10 px-3 py-2'>
                      <p className='font-medium text-foreground'>{r.title}</p>
                      <p className='font-mono text-label text-muted-foreground'>{r.slug}</p>
                      {r.reason && <p className='mt-1 text-label leading-[1.5] text-body'>{r.reason}</p>}
                      <div className='mt-2 flex items-center gap-2'>
                        <Button
                          size='sm'
                          onClick={() => void decide(r.id, true)}
                          disabled={busy}
                          className='h-[26px] gap-1.5 text-label font-semibold'
                        >
                          <Check size={12} strokeWidth={2.5} />
                          Approve &amp; publish
                        </Button>
                        <Button
                          size='sm'
                          variant='outline'
                          onClick={() => void decide(r.id, false)}
                          disabled={busy}
                          className='h-[26px] gap-1.5 text-label font-semibold'
                        >
                          <X size={12} strokeWidth={2.5} />
                          Reject
                        </Button>
                      </div>
                    </li>
                  ))}

                  {decided.map((r) => (
                    <li
                      key={r.id}
                      className='flex items-start justify-between gap-3 rounded-[6px] border border-border/60 px-3 py-2'
                    >
                      <div className='min-w-0'>
                        <p className='truncate text-body'>{r.title}</p>
                        {r.error && <p className='mt-0.5 text-label leading-[1.5] text-destructive'>{r.error}</p>}
                      </div>
                      <Outcome state={r.state} />
                    </li>
                  ))}
                </ul>
              )}
            </div>
          </>
        )}
      </div>
    </section>
  );
}

// ─── Pieces ───────────────────────────────────────────────────────────────────

const OUTCOMES = {
  publishing: { label: 'Publishing…', className: 'text-warning' },
  published: { label: 'Published', className: 'text-success' },
  rejected: { label: 'Rejected', className: 'text-muted-foreground' },
  failed: { label: 'Failed', className: 'text-destructive' },
} as const;

function Outcome({ state }: { state: PublishState }) {
  if (state === 'awaiting_approval') return null;
  const { label, className } = OUTCOMES[state];
  return <span className={cn('shrink-0 text-label font-semibold', className)}>{label}</span>;
}

function Secret({
  label,
  value,
  masked,
  onToggle,
}: {
  label: string;
  value: string;
  masked?: boolean;
  onToggle?: () => void;
}) {
  return (
    <div className='flex items-center justify-between gap-3'>
      <span className='shrink-0 text-muted-foreground'>{label}</span>
      <div className='flex min-w-0 items-center gap-1'>
        <span className='truncate font-mono text-label text-body'>{masked ? '•'.repeat(24) : value}</span>
        {onToggle && (
          <button
            type='button'
            onClick={onToggle}
            aria-label={masked ? `Show ${label}` : `Hide ${label}`}
            className='shrink-0 rounded p-1 text-muted-foreground transition-colors hover:text-body active:scale-95'
          >
            {masked ? <Eye size={12} strokeWidth={2} /> : <EyeOff size={12} strokeWidth={2} />}
          </button>
        )}
        <CopyButton label={`Copy ${label.toLowerCase()}`} value={value} iconOnly />
      </div>
    </div>
  );
}

function CopyButton({ label, value, iconOnly }: { label: string; value: string; iconOnly?: boolean }) {
  const [copied, setCopied] = useState(false);

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(value);
      setCopied(true);
      setTimeout(() => setCopied(false), 1800);
    } catch {
      // A denied clipboard leaves the value on screen to copy by hand.
    }
  };

  const mark = copied ? (
    <Check size={12} strokeWidth={2.5} className='text-success' />
  ) : (
    <Copy size={12} strokeWidth={2} />
  );

  if (iconOnly) {
    return (
      <button
        type='button'
        onClick={() => void copy()}
        aria-label={label}
        className='shrink-0 rounded p-1 text-muted-foreground transition-colors hover:text-body active:scale-95'
      >
        {mark}
      </button>
    );
  }

  return (
    <Button
      variant='outline'
      size='sm'
      onClick={() => void copy()}
      className='h-[28px] gap-1.5 text-label font-semibold'
    >
      {mark}
      {copied ? 'Copied' : label}
    </Button>
  );
}

/// The block an MCP client needs to reach this app, in the shape Claude Desktop
/// and Claude Code both read. Takes the token separately so it cannot be called
/// before one exists.
function clientConfig(endpoint: string, token: string): string {
  return JSON.stringify(
    {
      mcpServers: {
        'blog-cms': {
          type: 'http',
          url: endpoint,
          headers: { Authorization: `Bearer ${token}` },
        },
      },
    },
    null,
    2,
  );
}
