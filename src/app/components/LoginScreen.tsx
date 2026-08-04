"use client";

import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";

// Full-screen Cloudflare credentials form shown when there's no valid session.
export function LoginScreen({ onAuthed }: { onAuthed: () => void }) {
  const [accountId, setAccountId] = useState("");
  const [apiToken, setApiToken] = useState("");
  const [r2Bucket, setR2Bucket] = useState("");
  const [d1Id, setD1Id] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const ready = accountId && apiToken && r2Bucket && d1Id;

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!ready || busy) return;
    const { invoke, isTauri } = await import("@tauri-apps/api/core");
    if (!isTauri()) return;
    setBusy(true);
    setError(null);
    try {
      await invoke("save_credentials", {
        accountId,
        apiToken,
        r2Bucket,
        d1DatabaseId: d1Id,
      });
      const status = await invoke<{ authenticated: boolean }>("session_status");
      if (status.authenticated) {
        onAuthed();
      } else {
        setError("Saved, but the API token didn't verify. Check the token and try again.");
      }
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="flex h-screen items-center justify-center bg-zinc-50 dark:bg-[#0a0a0a] p-6">
      <form
        onSubmit={submit}
        className="w-full max-w-[380px] space-y-4 rounded-[10px] border border-zinc-200 dark:border-white/[0.08] bg-white dark:bg-[#161616] p-6 shadow-sm"
      >
        <div className="space-y-1">
          <h1 className="text-[16px] font-semibold text-zinc-900 dark:text-zinc-50">
            Connect to Cloudflare
          </h1>
          <p className="text-[12px] text-zinc-500 dark:text-zinc-400">
            Enter your Cloudflare credentials to use R2 and D1. They&rsquo;re stored locally on this
            device.
          </p>
        </div>

        <div className="space-y-2.5">
          <Field label="Account ID" value={accountId} onChange={setAccountId} placeholder="Cloudflare account id" />
          <Field label="API Token" value={apiToken} onChange={setApiToken} placeholder="R2 + D1 edit token" type="password" />
          <Field label="R2 Bucket" value={r2Bucket} onChange={setR2Bucket} placeholder="bucket name" />
          <Field label="D1 Database ID" value={d1Id} onChange={setD1Id} placeholder="database id" />
        </div>

        {error && <p className="text-[12px] font-medium text-red-600 dark:text-red-400">{error}</p>}

        <Button type="submit" disabled={!ready || busy} className="w-full h-[34px] text-[13px] font-semibold">
          {busy ? "Connecting…" : "Connect"}
        </Button>
      </form>
    </div>
  );
}

function Field({
  label,
  value,
  onChange,
  placeholder,
  type = "text",
}: {
  label: string;
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
  type?: string;
}) {
  return (
    <label className="block space-y-1">
      <span className="text-[11px] font-semibold text-zinc-600 dark:text-zinc-400">{label}</span>
      <Input
        type={type}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
        className="h-[32px] text-[13px]"
      />
    </label>
  );
}
