'use client';

import { useCallback, useEffect, useState } from 'react';
import { ImageOff, Loader2, X } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { cn } from '@/lib/utils';

export interface MediaEntry {
  /** R2 key, e.g. `media/<uuid>.avif`. */
  key: string;
  name: string;
  size: number;
}

/// Picker over the media library, opened by the editor's "Insert media" button.
///
/// The library is a reusable pool under `media/`, separate from a post's own
/// objects. Choosing an entry stages a local copy of it for the post, so it
/// travels the same publish path as a dropped image and ends up under the
/// post's own prefix — the same image can back several posts.
export function MediaPicker({
  open,
  onClose,
  onPick,
}: {
  open: boolean;
  onClose: () => void;
  onPick: (entry: MediaEntry) => void;
}) {
  const [items, setItems] = useState<MediaEntry[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    const { invoke, isTauri } = await import('@tauri-apps/api/core');
    if (!isTauri()) return;
    setLoading(true);
    setError(null);
    try {
      setItems(await invoke<MediaEntry[]>('list_media'));
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (open) void load();
  }, [open, load]);

  // Escape closes, matching the editor's other transient surfaces.
  useEffect(() => {
    if (!open) return;
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [open, onClose]);

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
        <div className='flex items-center justify-between border-b border-zinc-100 dark:border-white/[0.05] px-4 py-3'>
          <h2 className='text-[13px] font-semibold text-zinc-800 dark:text-zinc-200'>Select from Media library</h2>
          <Button
            variant='ghost'
            size='icon'
            aria-label='Close'
            onClick={onClose}
            className='size-[26px] rounded-[5px] text-zinc-400 dark:text-zinc-500'
          >
            <X size={13} strokeWidth={2} />
          </Button>
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
          ) : (
            <ul className='grid grid-cols-2 gap-2 sm:grid-cols-3'>
              {items.map((item) => (
                <li key={item.key}>
                  <button
                    onClick={() => onPick(item)}
                    title={item.name}
                    className={cn(
                      'w-full rounded-[6px] border px-2.5 py-2 text-left',
                      'border-zinc-200 dark:border-white/[0.07]',
                      'hover:bg-zinc-50 dark:hover:bg-white/[0.04]',
                      'active:scale-[0.98] transition-[background-color,transform] duration-100',
                    )}
                  >
                    <span className='block truncate font-mono text-[11px] text-zinc-700 dark:text-zinc-300'>
                      {item.name}
                    </span>
                    <span className='mt-0.5 block text-[10px] tabular-nums text-zinc-400 dark:text-zinc-600'>
                      {formatBytes(item.size)}
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

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const mb = bytes / (1024 * 1024);
  return mb < 1 ? `${(bytes / 1024).toFixed(0)} KB` : `${mb.toFixed(1)} MB`;
}
