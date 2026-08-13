"use client";

import { useCallback, useEffect, useState } from "react";
import { Check, Copy, Eye, EyeOff, KeyRound, Plug, ShieldCheck, TriangleAlert, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";

type McpStatus = {
  enabled: boolean;
  running: boolean;
  port: number;
  endpoint: string;
  /// Null until the server has been started for the first time — nothing is
  /// issued just for opening this screen.
  token: string | null;
};

type PublishState = "awaiting_approval" | "rejected" | "published" | "failed";

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
  const [status, setStatus]       = useState<McpStatus | null>(null);
  const [requests, setRequests]   = useState<PublishRequest[]>([]);
  const [port, setPort]           = useState("");
  const [busy, setBusy]           = useState(false);
  const [error, setError]         = useState<string | null>(null);
  const [reveal, setReveal]       = useState(false);
  const [available, setAvailable] = useState(true);

  const refresh = useCallback(async () => {
    const { invoke, isTauri } = await import("@tauri-apps/api/core");
    if (!isTauri()) {
      setAvailable(false);
      return;
    }
    try {
      const [next, queue] = await Promise.all([
        invoke<McpStatus>("mcp_status"),
        invoke<PublishRequest[]>("mcp_list_publish_requests"),
      ]);
      setStatus(next);
      setRequests(queue);
      // Only seed the input while it is untouched, so a refresh cannot
      // overwrite a port being typed.
      setPort((prev) => (prev === "" ? String(next.port) : prev));
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
      const { isTauri } = await import("@tauri-apps/api/core");
      if (!isTauri()) return;
      const { listen } = await import("@tauri-apps/api/event");
      const un = await listen<PublishRequest[]>("mcp://publish-requests-changed", (e) => {
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
      const { invoke, isTauri } = await import("@tauri-apps/api/core");
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
      const next = await invoke<McpStatus>("mcp_configure", { enabled, port: nextPort });
      setStatus(next);
      setPort(String(next.port));
    });

  const regenerate = () =>
    run(async (invoke) => {
      setStatus(await invoke<McpStatus>("mcp_regenerate_token"));
    });

  const decide = (id: string, approve: boolean) =>
    run(async (invoke) => {
      await invoke<PublishRequest>(approve ? "mcp_approve_publish" : "mcp_reject_publish", {
        requestId: id,
      });
      setRequests(await invoke<PublishRequest[]>("mcp_list_publish_requests"));
    });

  const parsedPort = Number.parseInt(port, 10);
  const portValid  = Number.isInteger(parsedPort) && parsedPort >= 1024 && parsedPort <= 65535;
  const portDirty  = status !== null && portValid && parsedPort !== status.port;

  const pending = requests.filter((r) => r.state === "awaiting_approval");
  const settled = requests.filter((r) => r.state !== "awaiting_approval");

  return (
    <section className="rounded-[8px] border border-zinc-200 dark:border-white/[0.07] bg-white dark:bg-[#161616]">
      <div className="flex items-center justify-between gap-3 px-4 py-3 border-b border-zinc-100 dark:border-white/[0.05]">
        <div>
          <h2 className="text-[13px] font-semibold text-zinc-800 dark:text-zinc-200">MCP server</h2>
          <p className="mt-0.5 text-[11px] text-zinc-500 dark:text-zinc-600">
            Let an AI assistant read and draft your posts. Publishing still needs your approval.
          </p>
        </div>
        <span
          className={cn(
            "flex shrink-0 items-center gap-1.5 rounded-full px-2 py-[3px] text-[10px] font-bold uppercase tracking-[0.08em]",
            status?.running
              ? "bg-emerald-50 text-emerald-700 dark:bg-emerald-500/10 dark:text-emerald-400"
              : "bg-zinc-100 text-zinc-500 dark:bg-white/[0.05] dark:text-zinc-500",
          )}
        >
          <span
            className={cn(
              "size-[5px] rounded-full",
              status?.running ? "bg-emerald-500" : "bg-zinc-400 dark:bg-zinc-600",
            )}
          />
          {status?.running ? "Running" : "Stopped"}
        </span>
      </div>

      <div className="px-4 py-3 space-y-4 text-[12px]">
        {!available ? (
          <p className="text-zinc-400 dark:text-zinc-600">The MCP server is unavailable in the browser.</p>
        ) : (
          <>
            {error && (
              <p className="flex items-start gap-1.5 text-[11px] leading-[1.5] text-red-600 dark:text-red-400">
                <TriangleAlert size={12} strokeWidth={2} className="mt-[2px] shrink-0" />
                {error}
              </p>
            )}

            <div className="flex items-center gap-2">
              <Button
                size="sm"
                variant={status?.enabled ? "outline" : "default"}
                onClick={() => void configure(!status?.enabled, portValid ? parsedPort : DEFAULT_PORT)}
                disabled={busy || !portValid}
                className="h-[30px] gap-1.5 text-[12px] font-semibold"
              >
                <Plug size={13} strokeWidth={2} />
                {status?.enabled ? "Turn off" : "Turn on"}
              </Button>

              <div className="flex items-center gap-1.5">
                <label className="text-zinc-500 dark:text-zinc-500">Port</label>
                <Input
                  value={port}
                  onChange={(e) => setPort(e.target.value)}
                  spellCheck={false}
                  inputMode="numeric"
                  className="h-[30px] w-[84px] font-mono text-[12px]"
                />
              </div>

              {portDirty && (
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() => void configure(status?.enabled ?? false, parsedPort)}
                  disabled={busy}
                  className="h-[30px] text-[12px] font-semibold"
                >
                  Apply
                </Button>
              )}
              {port !== "" && !portValid && (
                <span className="text-[11px] text-red-600 dark:text-red-400">1024–65535</span>
              )}
            </div>

            {status && (
              <div className="space-y-2 rounded-[6px] border border-zinc-100 dark:border-white/[0.05] bg-zinc-50 dark:bg-white/[0.02] px-3 py-2.5">
                <Secret label="Endpoint" value={status.endpoint} />
                {status.token ? (
                  <Secret
                    label="Token"
                    value={status.token}
                    masked={!reveal}
                    onToggle={() => setReveal((v) => !v)}
                  />
                ) : (
                  <div className="flex items-center justify-between gap-3">
                    <span className="shrink-0 text-zinc-500 dark:text-zinc-500">Token</span>
                    <span className="truncate text-[11px] text-zinc-400 dark:text-zinc-600">
                      Issued when you first turn the server on
                    </span>
                  </div>
                )}
                {status.token && (
                  <div className="flex flex-wrap items-center gap-2 pt-1">
                    <CopyButton label="Copy client config" value={clientConfig(status.endpoint, status.token)} />
                    <Button
                      variant="outline"
                      size="sm"
                      onClick={() => void regenerate()}
                      disabled={busy}
                      className="h-[28px] gap-1.5 text-[11px] font-semibold"
                    >
                      <KeyRound size={12} strokeWidth={2} />
                      New token
                    </Button>
                  </div>
                )}
                <p className="text-[11px] leading-[1.5] text-zinc-500 dark:text-zinc-600">
                  Paste the config into your MCP client. The endpoint listens on this machine only, and
                  rejects any request without the token.
                </p>
              </div>
            )}

            <div className="space-y-2 border-t border-zinc-100 dark:border-white/[0.05] pt-3">
              <div className="flex items-center gap-1.5">
                <ShieldCheck size={13} strokeWidth={2} className="text-zinc-400 dark:text-zinc-600" />
                <h3 className="text-[12px] font-semibold text-zinc-700 dark:text-zinc-300">Publish approvals</h3>
                {pending.length > 0 && (
                  <span className="rounded-full bg-amber-100 px-1.5 py-[1px] text-[10px] font-bold text-amber-700 dark:bg-amber-500/15 dark:text-amber-400">
                    {pending.length}
                  </span>
                )}
              </div>

              {requests.length === 0 ? (
                <p className="text-[11px] leading-[1.5] text-zinc-500 dark:text-zinc-600">
                  Nothing waiting. When an assistant asks to publish a post, it appears here for you to
                  approve.
                </p>
              ) : (
                <ul className="space-y-1.5">
                  {pending.map((r) => (
                    <li
                      key={r.id}
                      className="rounded-[6px] border border-amber-200 bg-amber-50/60 px-3 py-2 dark:border-amber-500/20 dark:bg-amber-500/[0.06]"
                    >
                      <p className="font-medium text-zinc-800 dark:text-zinc-200">{r.title}</p>
                      <p className="font-mono text-[11px] text-zinc-500 dark:text-zinc-500">{r.slug}</p>
                      {r.reason && (
                        <p className="mt-1 text-[11px] leading-[1.5] text-zinc-600 dark:text-zinc-400">
                          {r.reason}
                        </p>
                      )}
                      <div className="mt-2 flex items-center gap-2">
                        <Button
                          size="sm"
                          onClick={() => void decide(r.id, true)}
                          disabled={busy}
                          className="h-[26px] gap-1.5 text-[11px] font-semibold"
                        >
                          <Check size={12} strokeWidth={2.5} />
                          Approve &amp; publish
                        </Button>
                        <Button
                          size="sm"
                          variant="outline"
                          onClick={() => void decide(r.id, false)}
                          disabled={busy}
                          className="h-[26px] gap-1.5 text-[11px] font-semibold"
                        >
                          <X size={12} strokeWidth={2.5} />
                          Reject
                        </Button>
                      </div>
                    </li>
                  ))}

                  {settled.map((r) => (
                    <li
                      key={r.id}
                      className="flex items-start justify-between gap-3 rounded-[6px] border border-zinc-100 px-3 py-2 dark:border-white/[0.05]"
                    >
                      <div className="min-w-0">
                        <p className="truncate text-zinc-600 dark:text-zinc-400">{r.title}</p>
                        {r.error && (
                          <p className="mt-0.5 text-[11px] leading-[1.5] text-red-600 dark:text-red-400">
                            {r.error}
                          </p>
                        )}
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
  published: { label: "Published", className: "text-emerald-600 dark:text-emerald-500" },
  rejected:  { label: "Rejected",  className: "text-zinc-400 dark:text-zinc-600" },
  failed:    { label: "Failed",    className: "text-red-600 dark:text-red-400" },
} as const;

function Outcome({ state }: { state: PublishState }) {
  if (state === "awaiting_approval") return null;
  const { label, className } = OUTCOMES[state];
  return <span className={cn("shrink-0 text-[11px] font-semibold", className)}>{label}</span>;
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
    <div className="flex items-center justify-between gap-3">
      <span className="shrink-0 text-zinc-500 dark:text-zinc-500">{label}</span>
      <div className="flex min-w-0 items-center gap-1">
        <span className="truncate font-mono text-[11px] text-zinc-700 dark:text-zinc-300">
          {masked ? "•".repeat(24) : value}
        </span>
        {onToggle && (
          <button
            type="button"
            onClick={onToggle}
            aria-label={masked ? `Show ${label}` : `Hide ${label}`}
            className="shrink-0 rounded p-1 text-zinc-400 transition-colors hover:text-zinc-700 active:scale-95 dark:text-zinc-600 dark:hover:text-zinc-300"
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
    <Check size={12} strokeWidth={2.5} className="text-emerald-600 dark:text-emerald-500" />
  ) : (
    <Copy size={12} strokeWidth={2} />
  );

  if (iconOnly) {
    return (
      <button
        type="button"
        onClick={() => void copy()}
        aria-label={label}
        className="shrink-0 rounded p-1 text-zinc-400 transition-colors hover:text-zinc-700 active:scale-95 dark:text-zinc-600 dark:hover:text-zinc-300"
      >
        {mark}
      </button>
    );
  }

  return (
    <Button
      variant="outline"
      size="sm"
      onClick={() => void copy()}
      className="h-[28px] gap-1.5 text-[11px] font-semibold"
    >
      {mark}
      {copied ? "Copied" : label}
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
        "blog-cms": {
          type: "http",
          url: endpoint,
          headers: { Authorization: `Bearer ${token}` },
        },
      },
    },
    null,
    2,
  );
}
