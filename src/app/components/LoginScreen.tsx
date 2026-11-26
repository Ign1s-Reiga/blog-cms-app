'use client';

import { useState } from 'react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';

// D1 database ids are UUIDs; the API rejects anything else.
const UUID_RE = /^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$/;

// Full-screen Cloudflare credentials form shown when there's no valid session.
export function LoginScreen({ onAuthed }: { onAuthed: () => void }) {
  const [accountId, setAccountId] = useState('');
  const [apiToken, setApiToken] = useState('');
  const [r2Bucket, setR2Bucket] = useState('');
  const [d1Id, setD1Id] = useState('');
  const [r2PublicUrl, setR2PublicUrl] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const ready = accountId && apiToken && r2Bucket && d1Id && r2PublicUrl;

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!ready || busy) return;
    if (!UUID_RE.test(d1Id.trim())) {
      setError("D1 Database ID must be the database's UUID (8-4-4-4-12 hex) — not its name.");
      return;
    }
    // This is written into published Markdown as the base for image links, so a
    // typo here silently breaks every image on the blog rather than failing here.
    if (!/^https?:\/\/[^\s/]+/i.test(r2PublicUrl.trim())) {
      setError("R2 Public URL must be the bucket's public origin, e.g. https://cdn.example.com");
      return;
    }
    const { invoke, isTauri } = await import('@tauri-apps/api/core');
    if (!isTauri()) return;
    setBusy(true);
    setError(null);
    try {
      // Save and proceed. We don't gate login on Cloudflare's token-verify
      // endpoint — it rejects some valid, narrowly-scoped/account-owned tokens.
      // A bad token surfaces as a clear error when the app talks to R2/D1.
      await invoke('save_credentials', {
        accountId,
        apiToken,
        r2Bucket,
        d1DatabaseId: d1Id,
        r2PublicUrl,
      });
      onAuthed();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div
      role='dialog'
      aria-modal='true'
      aria-label='Sign in to Cloudflare'
      className='fixed inset-0 z-50 flex items-center justify-center bg-black/40 backdrop-blur-sm p-6'
    >
      <form
        onSubmit={submit}
        className='w-full max-w-[380px] space-y-4 rounded-[10px] border border-border bg-card p-6 shadow-xl'
      >
        <div className='space-y-1'>
          <h1 className='text-title font-semibold tracking-tight text-balance text-foreground'>
            Connect to Cloudflare
          </h1>
          <p className='text-meta text-muted-foreground'>
            Enter your Cloudflare credentials to use R2 and D1. Your API token is kept in your device&rsquo;s keychain.
          </p>
        </div>

        <div className='space-y-2.5'>
          {/* Focus starts here: the app behind this is inert, so the form is
              the only place a keyboard can go, and it should not have to look
              for it. */}
          <Field
            label='Account ID'
            value={accountId}
            onChange={setAccountId}
            placeholder='Cloudflare account id'
            autoFocus
          />
          <Field
            label='API Token'
            value={apiToken}
            onChange={setApiToken}
            placeholder='R2 + D1 edit token'
            type='password'
          />
          <Field label='R2 Bucket' value={r2Bucket} onChange={setR2Bucket} placeholder='bucket name' />
          <Field label='D1 Database ID' value={d1Id} onChange={setD1Id} placeholder='database id' />
          <Field
            label='R2 Public URL'
            value={r2PublicUrl}
            onChange={setR2PublicUrl}
            placeholder='https://cdn.example.com'
          />
        </div>

        {error && (
          <p role='alert' className='text-meta font-medium text-destructive'>
            {error}
          </p>
        )}

        <Button type='submit' disabled={!ready || busy} className='w-full h-[34px] text-ui font-semibold'>
          {busy ? 'Connecting…' : 'Connect'}
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
  type = 'text',
  autoFocus,
}: {
  label: string;
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
  type?: string;
  autoFocus?: boolean;
}) {
  return (
    <label className='block space-y-1'>
      <span className='text-label font-semibold text-body'>{label}</span>
      <Input
        type={type}
        autoFocus={autoFocus}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
        className='h-[32px] text-ui'
      />
    </label>
  );
}
