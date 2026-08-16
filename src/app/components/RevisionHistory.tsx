'use client';

import { useCallback, useEffect, useState } from 'react';
import { History, Loader2, RotateCcw, X } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { cn } from '@/lib/utils';

/// One entry as `list_revisions` returns it (see `RevisionSummary` in
/// `src-tauri/src/commands/local_db.rs`). Bodies are fetched one at a time.
type RevisionSummary = {
  id: number;
  post_id: number;
  title: string;
  origin: string;
  created_at: number;
  published: boolean;
  /// `null` when the snapshot carries no body — see the `body` column's docs.
  body_chars: number | null;
};

/// One entry in full, as `get_revision` returns it — the stored row itself, so
/// the shape is `post_revision::Model` rather than the summary plus a body.
type Revision = {
  id: number;
  post_id: number;
  title: string;
  excerpt: string | null;
  tags: string | null;
  published: boolean;
  /// `null` when the snapshot carries no body — see the `body` column's docs.
  body: string | null;
  origin: string;
  created_at: number;
};

/// What each snapshot was taken in front of. Every one of them reads as
/// "before …" because that is what a revision is here: the post as it stood
/// before the change named, never the result of it.
const ORIGIN_LABEL: Record<string, string> = {
  save: 'Before a save',
  publish: 'Before publishing',
  mcp: 'Before an MCP edit',
  restore: 'Before restoring',
  conflict_keep_remote: 'Before keeping the cloud copy',
};

function originLabel(origin: string): string {
  return ORIGIN_LABEL[origin] ?? `Before ${origin}`;
}

function formatWhen(unixSeconds: number): string {
  const then = new Date(unixSeconds * 1000);
  const elapsed = Math.round((Date.now() - then.getTime()) / 1000);
  if (elapsed < 60) return 'just now';
  if (elapsed < 3600) return `${Math.floor(elapsed / 60)} min ago`;
  if (elapsed < 86400) return `${Math.floor(elapsed / 3600)} h ago`;
  if (elapsed < 604800) return `${Math.floor(elapsed / 86400)} d ago`;
  return then.toISOString().slice(0, 10);
}

function formatSize(chars: number | null): string {
  if (chars === null) return 'metadata only';
  if (chars < 1000) return `${chars} chars`;
  return `${(chars / 1000).toFixed(1)}k chars`;
}

/// A post's saved versions, with a preview and a way back to any of them.
///
/// The list is what the backend recorded before each edit, so the top entry is
/// the version immediately before the current text — restoring it is the "undo
/// that last save" this exists for. Restoring never deletes anything: the
/// version being left is snapshotted on the way out, so a wrong restore is
/// itself one click from being undone.
export function RevisionHistory({
  open,
  postId,
  onClose,
  onRestored,
}: {
  open: boolean;
  /// `null` for a post that has never been saved, which has no history to show.
  postId: number | null;
  onClose: () => void;
  /// Called after a successful restore, so the editor can re-read the post it
  /// is showing — the text on screen is no longer what is stored.
  onRestored: () => void;
}) {
  const [items, setItems] = useState<RevisionSummary[]>([]);
  const [selected, setSelected] = useState<Revision | null>(null);
  const [loading, setLoading] = useState(false);
  const [restoring, setRestoring] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    if (postId === null) return;
    const { invoke, isTauri } = await import('@tauri-apps/api/core');
    if (!isTauri()) return;
    setLoading(true);
    setError(null);
    try {
      const rows = await invoke<RevisionSummary[]>('list_revisions', { postId });
      setItems(rows);
      setSelected(null);
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }, [postId]);

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

  const select = async (summary: RevisionSummary) => {
    const { invoke, isTauri } = await import('@tauri-apps/api/core');
    if (!isTauri()) return;
    try {
      setSelected(await invoke<Revision>('get_revision', { revisionId: summary.id }));
    } catch (err) {
      setError(String(err));
    }
  };

  const restore = async () => {
    if (!selected || restoring) return;
    const { invoke, isTauri } = await import('@tauri-apps/api/core');
    if (!isTauri()) return;
    setRestoring(true);
    setError(null);
    try {
      await invoke('restore_revision', { revisionId: selected.id });
      onRestored();
      onClose();
    } catch (err) {
      setError(String(err));
    } finally {
      setRestoring(false);
    }
  };

  if (!open) return null;

  return (
    <div
      role='dialog'
      aria-modal='true'
      aria-label='Version history'
      className='fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-6'
      onPointerDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div className='flex h-[76vh] w-full max-w-[900px] flex-col rounded-[8px] border border-zinc-200 dark:border-white/[0.08] bg-white dark:bg-[#161616]'>
        <div className='flex items-center justify-between border-b border-zinc-100 dark:border-white/[0.05] px-4 py-3'>
          <div className='flex items-baseline gap-2'>
            <h2 className='flex items-center gap-2 text-[13px] font-semibold text-zinc-800 dark:text-zinc-200'>
              <History size={13} strokeWidth={2} className='text-zinc-400 dark:text-zinc-500' />
              Version history
            </h2>
            <span className='text-[11px] text-zinc-400 dark:text-zinc-600'>
              each entry is the post as it stood before that change
            </span>
          </div>
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

        {error && (
          <p className='border-b border-red-100 bg-red-50 px-4 py-2 text-[12px] font-medium text-red-700 dark:border-red-500/20 dark:bg-red-500/[0.08] dark:text-red-400'>
            {error}
          </p>
        )}

        <div className='flex min-h-0 flex-1'>
          {/* Versions */}
          <ul className='w-[280px] shrink-0 overflow-y-auto border-r border-zinc-100 dark:border-white/[0.05] p-2'>
            {loading ? (
              <li className='flex items-center gap-2 p-3 text-[12px] text-zinc-500 dark:text-zinc-500'>
                <Loader2 size={13} strokeWidth={2} className='animate-spin' />
                Loading history…
              </li>
            ) : items.length === 0 ? (
              <li className='p-3 text-[12px] text-zinc-400 dark:text-zinc-600'>
                No earlier versions yet. One is kept each time this post is saved, published, or edited by an MCP
                client.
              </li>
            ) : (
              items.map((item) => (
                <li key={item.id}>
                  <button
                    type='button'
                    onClick={() => void select(item)}
                    aria-pressed={selected?.id === item.id}
                    className={cn(
                      'w-full rounded-[6px] px-2.5 py-2 text-left transition-colors active:scale-[0.99]',
                      selected?.id === item.id
                        ? 'bg-zinc-100 dark:bg-white/[0.06]'
                        : 'hover:bg-zinc-50 dark:hover:bg-white/[0.03]',
                    )}
                  >
                    <span className='flex items-baseline justify-between gap-2'>
                      <span className='truncate text-[12px] font-medium text-zinc-700 dark:text-zinc-300'>
                        {originLabel(item.origin)}
                      </span>
                      <span className='shrink-0 font-mono text-[10px] tabular-nums text-zinc-400 dark:text-zinc-600'>
                        {formatWhen(item.created_at)}
                      </span>
                    </span>
                    <span className='mt-0.5 block truncate text-[11px] text-zinc-500 dark:text-zinc-500'>
                      {item.title}
                    </span>
                    <span className='mt-0.5 block text-[10px] tabular-nums text-zinc-400 dark:text-zinc-600'>
                      {formatSize(item.body_chars)} · {item.published ? 'was live' : 'was a draft'}
                    </span>
                  </button>
                </li>
              ))
            )}
          </ul>

          {/* Preview of the selected version */}
          <div className='flex min-w-0 flex-1 flex-col'>
            {selected === null ? (
              <p className='p-4 text-[12px] text-zinc-400 dark:text-zinc-600'>Select a version to see what it held.</p>
            ) : (
              <>
                <div className='flex items-center justify-between gap-3 border-b border-zinc-100 dark:border-white/[0.05] px-4 py-2.5'>
                  <div className='min-w-0'>
                    <p className='truncate text-[13px] font-medium text-zinc-800 dark:text-zinc-200'>
                      {selected.title}
                    </p>
                    <p className='text-[11px] text-zinc-400 dark:text-zinc-600'>
                      {new Date(selected.created_at * 1000).toLocaleString()} · {originLabel(selected.origin)}
                    </p>
                  </div>
                  <Button
                    size='sm'
                    onClick={() => void restore()}
                    disabled={restoring}
                    title='Put the post back to this version. The version you are on now is kept, so this can be undone.'
                    className='h-[28px] shrink-0 gap-1.5 rounded-[5px] px-3 text-[12px] font-semibold'
                  >
                    <RotateCcw size={12} strokeWidth={2} />
                    {restoring ? 'Restoring…' : 'Restore this version'}
                  </Button>
                </div>
                <div className='min-h-0 flex-1 overflow-auto px-4 py-3'>
                  {selected.body === null ? (
                    <p className='text-[12px] text-zinc-400 dark:text-zinc-600'>
                      This version has no stored text — its body was not cached on this machine when the snapshot was
                      taken. Restoring it puts the title and tags back and leaves the text as it is.
                    </p>
                  ) : (
                    <pre className='whitespace-pre-wrap break-words font-mono text-[12px] leading-[1.7] text-zinc-600 dark:text-zinc-400'>
                      {selected.body}
                    </pre>
                  )}
                </div>
              </>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
