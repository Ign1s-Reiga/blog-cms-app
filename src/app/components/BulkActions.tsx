'use client';

import { useState } from 'react';
import { EyeOff, Loader2, RotateCcw, Send, Tag, Trash2, X } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { cn } from '@/lib/utils';

/// What a bulk run did to each post it touched.
export type BulkOutcome = {
  done: number;
  /// Named individually. A count of failures says something went wrong; this
  /// says which post and what it said, which is what makes it actionable.
  failed: { id: number; title: string; message: string }[];
  /// Posts a tag change deliberately left alone — their body is not on this
  /// machine, so the edit could not be marked and the next refresh would undo
  /// it. Mirrors `Skipped` in `src-tauri/src/commands/local_db.rs`.
  skipped: { id: number; title: string }[];
};

export type BulkAction = 'trash' | 'restore' | 'publish' | 'unpublish' | 'addTag' | 'removeTag';

/// The actions that reach Cloudflare, and therefore change what readers are
/// being served rather than only what this machine holds.
const REACHES_READERS: Record<BulkAction, boolean> = {
  trash: false,
  restore: false,
  publish: true,
  unpublish: true,
  addTag: false,
  removeTag: false,
};

const LABEL: Record<BulkAction, string> = {
  trash: 'Move to trash',
  restore: 'Restore',
  publish: 'Publish',
  unpublish: 'Unpublish',
  addTag: 'Add tag',
  removeTag: 'Remove tag',
};

/// Said in full before it happens, not after. Each names the count and what it
/// will actually do — the cloud ones especially, since those are the two a
/// person cannot simply undo by pressing the other button.
function describe(action: BulkAction, count: number, tag: string): string {
  const posts = `${count} post${count === 1 ? '' : 's'}`;
  switch (action) {
    case 'trash':
      return `Move ${posts} to the trash. A published post stays live — emptying the trash is the only step that is final.`;
    case 'restore':
      return `Take ${posts} out of the trash and back into the library.`;
    case 'publish':
      return `Publish ${posts} to the blog. This uploads each body and updates its row in D1, and readers see the result.`;
    case 'unpublish':
      return `Take ${posts} off the blog. The local copy stays; readers stop seeing them.`;
    case 'addTag':
      return `Add “${tag}” to ${posts}. A local edit — it goes up on the next push.`;
    case 'removeTag':
      return `Remove “${tag}” from ${posts}. A local edit — it goes up on the next push.`;
  }
}

/// The bar that appears once posts are selected, and the confirmation in front
/// of every action it offers.
export function BulkActions({
  selected,
  onClear,
  onRun,
  inTrash,
}: {
  selected: number[];
  onClear: () => void;
  onRun: (action: BulkAction, tag: string) => Promise<BulkOutcome>;
  /// The trash listing offers a different pair of actions from the library.
  inTrash: boolean;
}) {
  const [pending, setPending] = useState<BulkAction | null>(null);
  const [tag, setTag] = useState('');
  const [busy, setBusy] = useState(false);
  const [outcome, setOutcome] = useState<(BulkOutcome & { action: BulkAction }) | null>(null);

  if (selected.length === 0 && outcome === null) return null;

  const actions: BulkAction[] = inTrash ? ['restore'] : ['publish', 'unpublish', 'addTag', 'removeTag', 'trash'];

  const confirm = async () => {
    if (!pending || busy) return;
    setBusy(true);
    try {
      const result = await onRun(pending, tag.trim());
      setOutcome({ ...result, action: pending });
      setPending(null);
      setTag('');
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      {selected.length > 0 && (
        <div className='flex flex-wrap items-center gap-2 rounded-[6px] border border-zinc-200 bg-zinc-50 px-3 py-2 dark:border-white/[0.08] dark:bg-white/[0.03]'>
          <span className='text-[12px] font-medium text-zinc-700 dark:text-zinc-300'>{selected.length} selected</span>
          <span className='text-zinc-300 dark:text-zinc-700'>·</span>
          {actions.map((action) => (
            <Button
              key={action}
              variant='ghost'
              size='sm'
              onClick={() => setPending(action)}
              className={cn(
                'h-[26px] gap-1.5 px-2 text-[11px] font-medium',
                action === 'trash' && 'text-red-600 hover:bg-red-50 dark:text-red-400 dark:hover:bg-red-500/[0.1]',
              )}
            >
              {action === 'trash' && <Trash2 size={11} strokeWidth={2} />}
              {action === 'restore' && <RotateCcw size={11} strokeWidth={2} />}
              {action === 'publish' && <Send size={11} strokeWidth={2} />}
              {action === 'unpublish' && <EyeOff size={11} strokeWidth={2} />}
              {(action === 'addTag' || action === 'removeTag') && <Tag size={11} strokeWidth={2} />}
              {LABEL[action]}
            </Button>
          ))}
          <button
            type='button'
            onClick={onClear}
            className='ml-auto rounded-[4px] px-1.5 py-0.5 text-[11px] text-zinc-500 underline underline-offset-2 transition-colors hover:bg-zinc-100 dark:text-zinc-500 dark:hover:bg-white/[0.06]'
          >
            Clear selection
          </button>
        </div>
      )}

      {/* One confirmation for the batch, stating the count and the effect. */}
      {pending && (
        <div
          role='dialog'
          aria-modal='true'
          aria-label={`Confirm ${LABEL[pending].toLowerCase()}`}
          className='fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-6'
          onPointerDown={(e) => {
            if (e.target === e.currentTarget && !busy) setPending(null);
          }}
        >
          <div className='w-full max-w-[480px] rounded-[8px] border border-zinc-200 bg-white p-4 dark:border-white/[0.08] dark:bg-[#161616]'>
            <h2 className='text-[13px] font-semibold text-zinc-800 dark:text-zinc-200'>{LABEL[pending]}</h2>
            <p className='mt-2 text-[12px] leading-[1.6] text-zinc-600 dark:text-zinc-400'>
              {describe(pending, selected.length, tag.trim() || '…')}
            </p>

            {(pending === 'addTag' || pending === 'removeTag') && (
              <Input
                autoFocus
                value={tag}
                onChange={(e) => setTag(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') void confirm();
                }}
                placeholder='Tag name'
                spellCheck={false}
                className='mt-3 h-[30px] font-mono text-[12px]'
              />
            )}

            {REACHES_READERS[pending] && (
              <p className='mt-2 text-[11px] leading-[1.6] text-amber-600 dark:text-amber-500'>
                This one reaches Cloudflare, once per post.
              </p>
            )}

            <div className='mt-4 flex items-center justify-end gap-2'>
              <Button
                variant='ghost'
                size='sm'
                onClick={() => setPending(null)}
                disabled={busy}
                className='h-[30px] text-[12px]'
              >
                Cancel
              </Button>
              <Button
                size='sm'
                onClick={() => void confirm()}
                disabled={busy || ((pending === 'addTag' || pending === 'removeTag') && tag.trim() === '')}
                className={cn(
                  'h-[30px] gap-1.5 text-[12px] font-semibold',
                  pending === 'trash' && 'bg-red-600 text-white hover:bg-red-700 dark:bg-red-600 dark:hover:bg-red-700',
                )}
              >
                {busy && <Loader2 size={12} strokeWidth={2} className='animate-spin' />}
                {busy ? 'Working…' : LABEL[pending]}
              </Button>
            </div>
          </div>
        </div>
      )}

      {/* What happened, per post where it did not work. Nothing is rolled back:
          the posts that succeeded stay done, and the ones that did not are
          named rather than folded into a count. */}
      {outcome && (
        <div className='space-y-2'>
          <div className='flex items-start gap-2 rounded-[6px] border border-zinc-200 bg-zinc-50 px-3 py-2 dark:border-white/[0.08] dark:bg-white/[0.03]'>
            <p className='flex-1 text-[12px] text-zinc-600 dark:text-zinc-400'>
              {LABEL[outcome.action]}: {outcome.done} done
              {outcome.failed.length > 0 && `, ${outcome.failed.length} failed`}
              {outcome.skipped.length > 0 && `, ${outcome.skipped.length} left alone`}.
            </p>
            <button
              type='button'
              onClick={() => setOutcome(null)}
              aria-label='Dismiss'
              className='shrink-0 rounded-[4px] p-0.5 text-zinc-400 transition-colors hover:bg-zinc-100 dark:text-zinc-500 dark:hover:bg-white/[0.06]'
            >
              <X size={12} strokeWidth={2} />
            </button>
          </div>

          {outcome.failed.map((f) => (
            <p key={f.id} className='px-3 text-[11px] leading-[1.6] text-red-600 dark:text-red-400'>
              <span className='font-medium'>{f.title}</span>: {f.message}
            </p>
          ))}

          {outcome.skipped.length > 0 && (
            <p className='px-3 text-[11px] leading-[1.6] text-amber-600 dark:text-amber-500'>
              Left unchanged because their text is not on this machine, so the edit could not be recorded and the next
              refresh would undo it:{' '}
              {outcome.skipped.map((p, i) => (
                <span key={p.id}>
                  {i > 0 && ', '}
                  <span className='font-medium'>{p.title}</span>
                </span>
              ))}
              . Open them once to bring the text down, then try again.
            </p>
          )}
        </div>
      )}
    </>
  );
}
