'use client';

import { useCallback, useEffect, useState } from 'react';
import { LogOut, TriangleAlert } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { McpCard } from '@/components/McpCard';
import { UpdateCard } from '@/components/UpdateCard';

type Creds = {
  account_id: string;
  r2_bucket: string;
  d1_database_id: string;
  r2_public_url: string;
  thumbnail_key_pattern: string;
  media_key_pattern: string;
  web_analytics_site_tag: string;
};

type SaveState = { kind: 'idle' } | { kind: 'saving' } | { kind: 'saved' } | { kind: 'error'; message: string };

export default function SettingsPage() {
  const [creds, setCreds] = useState<Creds | null>(null);
  const [loading, setLoading] = useState(true);

  // Editable copies, seeded once the stored values arrive.
  const [publicUrl, setPublicUrl] = useState('');
  const [thumbPat, setThumbPat] = useState('');
  const [mediaPat, setMediaPat] = useState('');
  const [save, setSave] = useState<SaveState>({ kind: 'idle' });

  const load = useCallback(async () => {
    const { invoke, isTauri } = await import('@tauri-apps/api/core');
    if (!isTauri()) {
      setLoading(false);
      return;
    }
    try {
      const c = await invoke<Creds | null>('get_credentials');
      setCreds(c);
      if (c) {
        setPublicUrl(c.r2_public_url);
        setThumbPat(c.thumbnail_key_pattern);
        setMediaPat(c.media_key_pattern);
      }
    } catch {
      // ignore
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const dirty =
    creds !== null &&
    (publicUrl !== creds.r2_public_url ||
      thumbPat !== creds.thumbnail_key_pattern ||
      mediaPat !== creds.media_key_pattern);

  const commit = async () => {
    const { invoke, isTauri } = await import('@tauri-apps/api/core');
    if (!isTauri() || save.kind === 'saving') return;
    setSave({ kind: 'saving' });
    try {
      // The Rust side validates the patterns too — a bad one doesn't fail at
      // publish, it just writes objects where nothing will look for them.
      await invoke('save_settings', {
        r2PublicUrl: publicUrl,
        thumbnailKeyPattern: thumbPat,
        mediaKeyPattern: mediaPat,
      });
      await load();
      setSave({ kind: 'saved' });
      setTimeout(() => setSave({ kind: 'idle' }), 3000);
    } catch (err) {
      setSave({ kind: 'error', message: String(err) });
    }
  };

  const signOut = async () => {
    const { invoke, isTauri } = await import('@tauri-apps/api/core');
    if (!isTauri()) return;
    try {
      await invoke('clear_credentials');
      window.location.reload(); // re-run the AuthGate → back to the login screen
    } catch {
      // ignore
    }
  };

  return (
    <main className='flex-1 overflow-y-auto p-6'>
      <div className='max-w-[1100px] space-y-6'>
        <div>
          <h1 className='text-[15px] font-semibold text-zinc-800 dark:text-zinc-200'>Settings</h1>
          <p className='text-[12px] text-zinc-500 dark:text-zinc-600'>Cloudflare connection and preferences.</p>
        </div>

        {/* Two equal columns. `items-start` keeps each card at its natural
            height — stretching the short Cloudflare card to match the much
            taller Media one would leave it mostly empty. */}
        <div className='grid items-start gap-6 lg:grid-cols-2'>
          {/* Left: connection, then the MCP endpoint. */}
          <div className='space-y-6'>
            <section className='rounded-[8px] border border-zinc-200 dark:border-white/[0.07] bg-white dark:bg-[#161616]'>
              <div className='px-4 py-3 border-b border-zinc-100 dark:border-white/[0.05]'>
                <h2 className='text-[13px] font-semibold text-zinc-800 dark:text-zinc-200'>Cloudflare</h2>
              </div>
              <div className='px-4 py-3 space-y-2 text-[12px]'>
                {loading ? (
                  <p className='text-zinc-400 dark:text-zinc-600'>Loading…</p>
                ) : creds ? (
                  <>
                    <Row label='Account ID' value={creds.account_id} />
                    <Row label='R2 Bucket' value={creds.r2_bucket} />
                    <Row label='D1 Database' value={creds.d1_database_id} />
                    {/* Chosen on the Analytics route, which is where the list
                        of sites can be fetched; shown here so the connection
                        card states every fact about it. */}
                    <Row
                      label='Web Analytics'
                      value={creds.web_analytics_site_tag || 'Not set — choose one on Analytics'}
                    />
                    <div className='pt-2'>
                      <Button
                        variant='outline'
                        size='sm'
                        onClick={signOut}
                        className='h-[30px] gap-1.5 text-[12px] font-semibold text-red-600 dark:text-red-400'
                      >
                        <LogOut size={13} strokeWidth={2} />
                        Sign out
                      </Button>
                    </div>
                  </>
                ) : (
                  <p className='text-zinc-400 dark:text-zinc-600'>Not connected.</p>
                )}
              </div>
            </section>

            {/* Outside the `creds` guard: drafting over MCP is local-only and
                works before Cloudflare is connected. */}
            <McpCard />
          </div>

          {/* Right: media settings, then updates. */}
          <div className='space-y-6'>
            {creds && (
              <section className='rounded-[8px] border border-zinc-200 dark:border-white/[0.07] bg-white dark:bg-[#161616]'>
                <div className='px-4 py-3 border-b border-zinc-100 dark:border-white/[0.05]'>
                  <h2 className='text-[13px] font-semibold text-zinc-800 dark:text-zinc-200'>Media</h2>
                  <p className='mt-0.5 text-[11px] text-zinc-500 dark:text-zinc-600'>
                    Where uploaded media is stored in R2, and the URL it is served from.
                  </p>
                </div>

                <div className='px-4 py-3 space-y-4 text-[12px]'>
                  <Field
                    label='R2 Public URL'
                    value={publicUrl}
                    onChange={setPublicUrl}
                    placeholder='https://cdn.example.com'
                    hint="Written into published posts as the base for image links. Must match the blog's R2_PUBLIC_URL."
                  />

                  <Field
                    label='Thumbnail key pattern'
                    value={thumbPat}
                    onChange={setThumbPat}
                    placeholder='posts/{slug}/thumbnail.{ext}'
                    hint='Supports {slug} and {ext}.'
                    warning="The blog derives this key from the slug alone, so it must match thumbnailKey in the blog's content.ts. A mismatch hides every thumbnail with no error anywhere."
                  />

                  <Field
                    label='Media key pattern'
                    value={mediaPat}
                    onChange={setMediaPat}
                    placeholder='posts/{slug}/{hash}.{ext}'
                    hint="Supports {slug}, {hash} and {ext}. Safe to change — published posts carry each image's full URL, so the blog never derives these. {hash} is required: without it, two images in one post overwrite each other."
                  />

                  <div className='flex items-center gap-3 pt-0.5'>
                    <Button
                      size='sm'
                      onClick={commit}
                      disabled={!dirty || save.kind === 'saving'}
                      className='h-[30px] text-[12px] font-semibold'
                    >
                      {save.kind === 'saving' ? 'Saving…' : 'Save changes'}
                    </Button>
                    {save.kind === 'saved' && (
                      <span className='text-[12px] font-medium text-emerald-600 dark:text-emerald-500'>Saved</span>
                    )}
                    {save.kind === 'error' && (
                      <span className='text-[12px] font-medium text-red-600 dark:text-red-400'>{save.message}</span>
                    )}
                  </div>
                </div>
              </section>
            )}

            <UpdateCard />
          </div>
        </div>
      </div>
    </main>
  );
}

function Row({ label, value }: { label: string; value: string }) {
  return (
    <div className='flex items-center justify-between gap-4'>
      <span className='text-zinc-500 dark:text-zinc-500'>{label}</span>
      <span className='max-w-[300px] truncate font-mono text-zinc-700 dark:text-zinc-300'>{value}</span>
    </div>
  );
}

function Field({
  label,
  value,
  onChange,
  placeholder,
  hint,
  warning,
}: {
  label: string;
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
  hint?: string;
  warning?: string;
}) {
  return (
    <div className='space-y-1.5'>
      <label className='block text-[12px] font-medium text-zinc-700 dark:text-zinc-300'>{label}</label>
      <Input
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
        spellCheck={false}
        className='h-[32px] font-mono text-[12px]'
      />
      {hint && <p className='text-[11px] leading-[1.5] text-zinc-500 dark:text-zinc-600'>{hint}</p>}
      {warning && (
        <p className='flex items-start gap-1.5 text-[11px] leading-[1.5] text-amber-600 dark:text-amber-500'>
          <TriangleAlert size={12} strokeWidth={2} className='mt-[2px] shrink-0' />
          {warning}
        </p>
      )}
    </div>
  );
}
