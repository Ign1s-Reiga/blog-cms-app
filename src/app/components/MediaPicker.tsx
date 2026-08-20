'use client';

import { useCallback, useEffect, useMemo, useState } from 'react';
import { ImageOff, Loader2, Play, Search, X } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { isVideo } from '@/lib/media';
import { cn } from '@/lib/utils';

export interface MediaEntry {
  /** R2 key, e.g. `media/<uuid>.avif`. */
  key: string;
  name: string;
  size: number;
}

/// A library entry with what the tile needs to show it.
type Previewable = MediaEntry & {
  /// Asset-protocol URL for the locally cached copy.
  src: string;
  video: boolean;
};

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

/// Picker over the media library, opened by the editor's "Insert media" button.
///
/// The library is a reusable pool under `media/`, separate from a post's own
/// objects. Choosing an entry stages a local copy of it for the post, so it
/// travels the same publish path as a dropped image and ends up under the
/// post's own prefix — the same image can back several posts.
///
/// Every entry is shown, not named. A list of file names asks the author to
/// remember which `3f2b8c.avif` was the one they wanted, and gives no way at all
/// to tell an image from a video before picking it — which mattered, because the
/// two are inserted differently.
export function MediaPicker({
  open,
  onClose,
  onPick,
}: {
  open: boolean;
  onClose: () => void;
  onPick: (entry: MediaEntry) => void;
}) {
  const [items, setItems] = useState<Previewable[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [query, setQuery] = useState('');

  const load = useCallback(async () => {
    const { invoke, isTauri, convertFileSrc } = await import('@tauri-apps/api/core');
    if (!isTauri()) return;
    setLoading(true);
    setError(null);
    try {
      const rows = await invoke<MediaEntry[]>('list_media');
      const { appDataDir, join } = await import('@tauri-apps/api/path');
      const base = await appDataDir();
      setItems(
        await Promise.all(
          rows.map(async (r) => ({
            ...r,
            // The key doubles as the local-relative cache path — `list_media`
            // has already brought every object down.
            src: convertFileSrc(await join(base, r.key)),
            video: isVideo(r.name),
          })),
        ),
      );
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (open) void load();
  }, [open, load]);

  // A stale query would hide most of the library the next time this opens.
  useEffect(() => {
    if (!open) setQuery('');
  }, [open]);

  // Escape closes, matching the editor's other transient surfaces.
  useEffect(() => {
    if (!open) return;
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [open, onClose]);

  const visible = useMemo(() => {
    const q = query.trim().toLowerCase();
    return q === '' ? items : items.filter((i) => i.name.toLowerCase().includes(q));
  }, [items, query]);

  if (!open) return null;

  return (
    <div
      role='dialog'
      aria-modal='true'
      aria-label='Select from Media library'
      className='fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-6'
      onPointerDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div className='flex max-h-[70vh] w-full max-w-[620px] flex-col rounded-[8px] border border-zinc-200 dark:border-white/[0.08] bg-white dark:bg-[#161616]'>
        <div className='flex items-center justify-between gap-3 border-b border-zinc-100 dark:border-white/[0.05] px-4 py-3'>
          <h2 className='shrink-0 text-[13px] font-semibold text-zinc-800 dark:text-zinc-200'>
            Select from Media library
          </h2>
          <div className='flex items-center gap-2'>
            {items.length > 0 && (
              <div className='relative'>
                <Search
                  size={12}
                  strokeWidth={2}
                  className='pointer-events-none absolute left-2 top-1/2 -translate-y-1/2 text-zinc-400 dark:text-zinc-600'
                />
                <Input
                  value={query}
                  onChange={(e) => setQuery(e.target.value)}
                  placeholder='Filter by name'
                  spellCheck={false}
                  className='h-[28px] w-[160px] pl-6 text-[12px]'
                />
              </div>
            )}
            <Button
              variant='ghost'
              size='icon'
              aria-label='Close'
              onClick={onClose}
              className='size-[26px] shrink-0 rounded-[5px] text-zinc-400 dark:text-zinc-500'
            >
              <X size={13} strokeWidth={2} />
            </Button>
          </div>
        </div>

        <div className='min-h-[120px] flex-1 overflow-y-auto p-3'>
          {loading ? (
            <p className='flex items-center gap-2 p-3 text-[12px] text-zinc-500 dark:text-zinc-500'>
              <Loader2 size={13} strokeWidth={2} className='animate-spin' />
              Loading media…
            </p>
          ) : error ? (
            <p className='p-3 text-[12px] text-red-600 dark:text-red-400'>{error}</p>
          ) : items.length === 0 ? (
            <p className='flex items-center gap-2 p-3 text-[12px] text-zinc-400 dark:text-zinc-600'>
              <ImageOff size={13} strokeWidth={1.8} />
              Nothing in the library yet — upload from the Media page.
            </p>
          ) : visible.length === 0 ? (
            <p className='p-3 text-[12px] text-zinc-400 dark:text-zinc-600'>
              Nothing in the library matches “{query}”.
            </p>
          ) : (
            <ul className='grid grid-cols-2 gap-2 sm:grid-cols-3'>
              {visible.map((item) => (
                <li key={item.key}>
                  <button
                    onClick={() => onPick(item)}
                    title={item.name}
                    className={cn(
                      'w-full overflow-hidden rounded-[6px] border text-left',
                      'border-zinc-200 dark:border-white/[0.07]',
                      'hover:border-zinc-300 dark:hover:border-white/[0.14]',
                      'active:scale-[0.98] transition-[border-color,transform] duration-100',
                    )}
                  >
                    <span className='relative flex aspect-[4/3] items-center justify-center overflow-hidden bg-zinc-50 dark:bg-white/[0.02]'>
                      {item.video ? (
                        <>
                          {/* No `controls`: the tile is a preview to pick from,
                              not a player. `metadata` is enough for a frame. */}
                          <video src={item.src} muted preload='metadata' className='size-full object-cover' />
                          <span className='absolute inset-0 flex items-center justify-center'>
                            <span className='flex size-[22px] items-center justify-center rounded-full bg-black/55 text-white'>
                              <Play size={11} strokeWidth={2.5} className='ml-[1px]' />
                            </span>
                          </span>
                        </>
                      ) : (
                        // eslint-disable-next-line @next/next/no-img-element
                        <img src={item.src} alt={item.name} loading='lazy' className='size-full object-cover' />
                      )}
                    </span>
                    <span className='block border-t border-zinc-100 px-2.5 py-1.5 dark:border-white/[0.05]'>
                      <span className='block truncate font-mono text-[11px] text-zinc-700 dark:text-zinc-300'>
                        {item.name}
                      </span>
                      <span className='mt-0.5 block text-[10px] tabular-nums text-zinc-400 dark:text-zinc-600'>
                        {formatBytes(item.size)}
                        {item.video && ' · video'}
                      </span>
                    </span>
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
      </div>
    </div>
  );
}
