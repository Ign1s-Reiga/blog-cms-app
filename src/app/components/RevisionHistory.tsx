'use client';

import { useCallback, useEffect, useState } from 'react';
import { History, Loader2, RotateCcw, X } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Dialog, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog';
import { useReturnFocus } from '@/lib/useReturnFocus';
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
  onBeforeRestore,
  onRestored,
}: {
  open: boolean;
  /// `null` for a post that has never been saved, which has no history to show.
  postId: number | null;
  onClose: () => void;
  /// Called before the restore, to write whatever is in the editor to disk.
  ///
  /// Restoring promises that the version being left is kept, and the version
  /// being left is the one on screen — not the one last written. Without this,
  /// edits made since the last save would be thrown away by the reload that
  /// follows a restore, and no snapshot would hold them: `restore_revision`
  /// snapshots what is *stored*, which is precisely what those edits are not.
  ///
  /// Rejecting cancels the restore, because the alternative is losing the work
  /// this call exists to protect.
  onBeforeRestore: () => Promise<void>;
  /// Called after a successful restore, so the editor can re-read the post it
  /// is showing — the text on screen is no longer what is stored.
  ///
  /// Awaited, and the panel stays open until it finishes. Closing first would
  /// hand back an editor still showing the pre-restore text, with a reload
  /// landing on it moments later: type into that gap and the reload overwrites
  /// what was typed. A reload that fails would leave the stale text on screen
  /// under a dialog that had already reported success.
  onRestored: () => Promise<void>;
}) {
  const [items, setItems] = useState<RevisionSummary[]>([]);
  const [selected, setSelected] = useState<Revision | null>(null);
  const [loading, setLoading] = useState(false);
  const [restoring, setRestoring] = useState(false);
  const returnFocus = useReturnFocus();
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

  /// Dismissing the panel, from wherever it is asked for — Escape, the X, the
  /// backdrop.
  ///
  /// Refused outright while a restore is running. The panel is what stands
  /// between the editor and work that is still in flight: closing it hands back
  /// an interactive editor whose text is about to be replaced by the reload,
  /// and anything typed in that window is overwritten without trace. It is a
  /// short wait, and the button says what it is waiting for.
  const dismiss = useCallback(() => {
    if (restoring) return;
    onClose();
  }, [restoring, onClose]);

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
      // What is on screen goes to disk first, so the restore's own snapshot
      // captures it and this is one click from being undone.
      await onBeforeRestore();
      await invoke('restore_revision', { revisionId: selected.id });
      // Only once the editor is showing the restored version is this finished.
      await onRestored();
      onClose();
    } catch (err) {
      setError(String(err));
    } finally {
      setRestoring(false);
    }
  };

  // Built on the Dialog primitive so focus moves into the panel, stays there,
  // and goes back to the button that opened it. Every way of dismissing it —
  // Escape, the backdrop, the X — reaches `dismiss`, so the refusal while a
  // restore runs holds for all of them. Escape and outside clicks are also
  // cancelled at the source then, so the primitive never starts closing a
  // panel that is going to stay.
  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!next) dismiss();
      }}
    >
      <DialogContent
        {...returnFocus}
        showCloseButton={false}
        onEscapeKeyDown={(e) => {
          if (restoring) e.preventDefault();
        }}
        onInteractOutside={(e) => {
          if (restoring) e.preventDefault();
        }}
        className='flex h-[76vh] w-full max-w-[min(900px,calc(100%-3rem))] flex-col gap-0 rounded-[8px] border border-border bg-card p-0 ring-0 sm:max-w-[min(900px,calc(100%-3rem))]'
      >
        <div className='flex items-center justify-between border-b border-border/60 px-4 py-3'>
          <div className='flex items-baseline gap-2'>
            <DialogTitle className='flex items-center gap-2 text-ui leading-normal font-semibold text-foreground'>
              <History size={13} strokeWidth={2} className='text-muted-foreground' />
              Version history
            </DialogTitle>
            <DialogDescription className='text-label text-muted-foreground'>
              each entry is the post as it stood before that change
            </DialogDescription>
          </div>
          <Button
            variant='ghost'
            size='icon'
            aria-label='Close'
            onClick={dismiss}
            disabled={restoring}
            className='size-[26px] rounded-[5px] text-muted-foreground'
          >
            <X size={13} strokeWidth={2} />
          </Button>
        </div>

        {error && (
          <p
            role='alert'
            className='border-b border-destructive/30 bg-destructive/10 px-4 py-2 text-meta font-medium text-destructive'
          >
            {error}
          </p>
        )}

        <div className='flex min-h-0 flex-1'>
          {/* Versions */}
          <ul className='w-[280px] shrink-0 overflow-y-auto border-r border-border/60 p-2'>
            {loading ? (
              <li className='flex items-center gap-2 p-3 text-meta text-muted-foreground'>
                <Loader2 size={13} strokeWidth={2} className='animate-spin' />
                Loading history…
              </li>
            ) : items.length === 0 ? (
              <li className='p-3 text-meta text-muted-foreground'>
                No earlier versions yet. One is kept each time this post is saved, published, or edited by an MCP
                client.
              </li>
            ) : (
              items.map((item) => (
                <li key={item.id}>
                  <button
                    type='button'
                    onClick={() => void select(item)}
                    // The version on show, of several — not a toggle, so
                    // `aria-current` rather than `aria-pressed`, which would
                    // announce every other entry as a switch that is off.
                    aria-current={selected?.id === item.id ? 'true' : undefined}
                    className={cn(
                      'w-full rounded-[6px] px-2.5 py-2 text-left transition-colors active:scale-[0.99]',
                      selected?.id === item.id ? 'bg-muted' : 'hover:bg-muted/50',
                    )}
                  >
                    <span className='flex items-baseline justify-between gap-2'>
                      <span className='truncate text-meta font-medium text-body'>{originLabel(item.origin)}</span>
                      <span className='shrink-0 font-mono text-label tabular-nums text-muted-foreground'>
                        {formatWhen(item.created_at)}
                      </span>
                    </span>
                    <span className='mt-0.5 block truncate text-label text-muted-foreground'>{item.title}</span>
                    <span className='mt-0.5 block text-label tabular-nums text-muted-foreground'>
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
              <p className='p-4 text-meta text-muted-foreground'>Select a version to see what it held.</p>
            ) : (
              <>
                <div className='flex items-center justify-between gap-3 border-b border-border/60 px-4 py-2.5'>
                  <div className='min-w-0'>
                    <p className='truncate text-ui font-medium text-foreground'>{selected.title}</p>
                    <p className='text-label text-muted-foreground'>
                      {new Date(selected.created_at * 1000).toLocaleString()} · {originLabel(selected.origin)}
                    </p>
                  </div>
                  <Button
                    size='sm'
                    onClick={() => void restore()}
                    disabled={restoring}
                    title='Put the post back to this version. The version you are on now is kept, so this can be undone.'
                    className='h-[28px] shrink-0 gap-1.5 rounded-[5px] px-3 text-meta font-semibold'
                  >
                    <RotateCcw size={12} strokeWidth={2} />
                    {restoring ? 'Restoring…' : 'Restore this version'}
                  </Button>
                </div>
                <div className='min-h-0 flex-1 overflow-auto px-4 py-3'>
                  {selected.body === null ? (
                    <p className='text-meta text-muted-foreground'>
                      This version has no stored text — its body was not cached on this machine when the snapshot was
                      taken. Restoring it puts the title and tags back and leaves the text as it is.
                    </p>
                  ) : (
                    <pre className='whitespace-pre-wrap break-words font-mono text-meta leading-[1.7] text-body'>
                      {selected.body}
                    </pre>
                  )}
                </div>
              </>
            )}
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
