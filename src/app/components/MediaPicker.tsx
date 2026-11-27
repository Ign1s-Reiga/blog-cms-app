'use client';

import { useCallback, useEffect, useMemo, useState } from 'react';
import { ImageOff, Loader2, Play, Search, X } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Dialog, DialogClose, DialogContent, DialogTitle } from '@/components/ui/dialog';
import { Input } from '@/components/ui/input';
import { isVideo } from '@/lib/media';
import { useReturnFocus } from '@/lib/useReturnFocus';
import { cn } from '@/lib/utils';
import { convertFileSrc, invoke, isTauri } from '@tauri-apps/api/core';
import { appDataDir, join } from '@tauri-apps/api/path';

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
  const returnFocus = useReturnFocus();

  const load = useCallback(async () => {
    if (!isTauri()) return;
    setLoading(true);
    setError(null);
    try {
      const rows = await invoke<MediaEntry[]>('list_media');
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

  const visible = useMemo(() => {
    const q = query.trim().toLowerCase();
    return q === '' ? items : items.filter((i) => i.name.toLowerCase().includes(q));
  }, [items, query]);

  // Built on the Dialog primitive rather than a hand-rolled overlay, because a
  // modal has to own the keyboard while it is up: focus moves into it, Tab
  // stays inside it, and closing hands focus back to the button that opened
  // it. Escape and a click on the backdrop both arrive here as a close.
  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!next) onClose();
      }}
    >
      <DialogContent
        {...returnFocus}
        showCloseButton={false}
        aria-describedby={undefined}
        className='flex max-h-[70vh] w-full max-w-[min(620px,calc(100%-3rem))] flex-col gap-0 rounded-[8px] border border-border bg-card p-0 ring-0 sm:max-w-[min(620px,calc(100%-3rem))]'
      >
        <div className='flex items-center justify-between gap-3 border-b border-border/60 px-4 py-3'>
          <DialogTitle className='shrink-0 text-ui leading-normal font-semibold text-foreground'>
            Select from Media library
          </DialogTitle>
          <div className='flex items-center gap-2'>
            {items.length > 0 && (
              <div className='relative'>
                <Search
                  size={12}
                  strokeWidth={2}
                  className='pointer-events-none absolute left-2 top-1/2 -translate-y-1/2 text-muted-foreground'
                />
                <Input
                  value={query}
                  onChange={(e) => setQuery(e.target.value)}
                  placeholder='Filter by name'
                  aria-label='Filter media by name'
                  spellCheck={false}
                  className='h-[28px] w-[160px] pl-6 text-meta'
                />
              </div>
            )}
            <DialogClose asChild>
              <Button
                variant='ghost'
                size='icon'
                aria-label='Close'
                className='size-[26px] shrink-0 rounded-[5px] text-muted-foreground'
              >
                <X size={13} strokeWidth={2} />
              </Button>
            </DialogClose>
          </div>
        </div>

        <div className='min-h-[120px] flex-1 overflow-y-auto p-3'>
          {loading ? (
            <p className='flex items-center gap-2 p-3 text-meta text-muted-foreground'>
              <Loader2 size={13} strokeWidth={2} className='animate-spin' />
              Loading media…
            </p>
          ) : error ? (
            <p role='alert' className='p-3 text-meta text-destructive'>
              {error}
            </p>
          ) : items.length === 0 ? (
            <p className='flex items-center gap-2 p-3 text-meta text-muted-foreground'>
              <ImageOff size={13} strokeWidth={1.8} />
              Nothing in the library yet — upload from the Media page.
            </p>
          ) : visible.length === 0 ? (
            <p className='p-3 text-meta text-muted-foreground'>Nothing in the library matches “{query}”.</p>
          ) : (
            <ul className='grid grid-cols-2 gap-2 sm:grid-cols-3'>
              {visible.map((item) => (
                <li key={item.key}>
                  <button
                    onClick={() => onPick(item)}
                    title={item.name}
                    className={cn(
                      'w-full overflow-hidden rounded-[6px] border text-left',
                      'border-border',
                      'hover:border-input',
                      'active:scale-[0.98] transition-[border-color,transform] duration-100',
                    )}
                  >
                    <span className='relative flex aspect-[4/3] items-center justify-center overflow-hidden bg-muted/50'>
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
                    <span className='block border-t border-border/60 px-2.5 py-1.5'>
                      <span className='block truncate font-mono text-label text-body'>{item.name}</span>
                      <span className='mt-0.5 block text-label tabular-nums text-muted-foreground'>
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
      </DialogContent>
    </Dialog>
  );
}
