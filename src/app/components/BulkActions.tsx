'use client';

import { useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { EyeOff, Loader2, RotateCcw, Send, Tag, Trash2, X } from 'lucide-react';
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { useReturnFocus } from '@/lib/useReturnFocus';
import { cn } from '@/lib/utils';

/// What a bulk run did to each post it touched.
export type BulkOutcome = {
  done: number;
  /// Named individually. A count of failures says something went wrong; this
  /// says which post and what it said, which is what makes it actionable.
  failed: { id: number; title: string; message: string }[];
  /// Posts a tag change deliberately left alone — the body here cannot stand in
  /// for the one the post actually has, so the edit could not be marked and the
  /// next refresh would undo it. Mirrors `Skipped` in
  /// `src-tauri/src/commands/local_db.rs`.
  skipped: { id: number; title: string; reason: SkipReason }[];
};

/// Mirrors `SkipReason`. Both end in the same advice — open the post once —
/// but for different reasons, and saying the wrong one sends the reader looking
/// for a problem they do not have.
export type SkipReason = 'body_not_cached' | 'body_stale';

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

/// One line on how a run went, for the panel and for the announcement alike.
function summarize(outcome: BulkOutcome & { action: BulkAction }): string {
  let line = `${LABEL[outcome.action]}: ${outcome.done} done`;
  if (outcome.failed.length > 0) line += `, ${outcome.failed.length} failed`;
  if (outcome.skipped.length > 0) line += `, ${outcome.skipped.length} left alone`;
  return `${line}.`;
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
  /// A run that failed as a whole, rather than post by post. Shown in the
  /// dialog, which stays open so it can be tried again or cancelled.
  const [runError, setRunError] = useState<string | null>(null);
  const tagInput = useRef<HTMLInputElement>(null);
  const returnFocus = useReturnFocus();

  // The action the confirmation is worded for, held past `pending` going back
  // to null. The dialog animates out after it is closed, and rendering its
  // contents from `pending` would blank them — or crash on `LABEL[null]` —
  // for the length of that animation.
  const [shown, setShown] = useState<BulkAction | null>(pending);
  if (pending !== null && pending !== shown) setShown(pending);

  if (selected.length === 0 && outcome === null) return null;

  const actions: BulkAction[] = inTrash ? ['restore'] : ['publish', 'unpublish', 'addTag', 'removeTag', 'trash'];

  const confirm = async () => {
    if (!pending || busy) return;
    // The same rule that disables the confirm button, here because Enter in
    // the tag field reaches this without going through the button.
    if ((pending === 'addTag' || pending === 'removeTag') && tag.trim() === '') return;
    setBusy(true);
    setRunError(null);
    try {
      const result = await onRun(pending, tag.trim());
      setOutcome({ ...result, action: pending });
      setPending(null);
      setTag('');
    } catch (err) {
      setRunError(String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      {selected.length > 0 && (
        <div className='flex flex-wrap items-center gap-2 rounded-[6px] border border-border bg-muted/50 px-3 py-2'>
          <span className='text-meta font-medium text-body'>{selected.length} selected</span>
          <span className='text-muted-foreground'>·</span>
          {actions.map((action) => (
            <Button
              key={action}
              variant='ghost'
              size='sm'
              onClick={() => setPending(action)}
              className={cn(
                'h-[26px] gap-1.5 px-2 text-label font-medium',
                action === 'trash' && 'text-destructive hover:bg-destructive/10',
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
            className='ml-auto rounded-[4px] px-1.5 py-0.5 text-label text-muted-foreground underline underline-offset-2 transition-colors hover:bg-muted'
          >
            Clear selection
          </button>
        </div>
      )}

      {/* One confirmation for the batch, stating the count and the effect.

          An alert dialog, so focus starts on Cancel rather than on the action
          and comes back to the bar when it closes. Nothing closes it while a
          run is in flight: the outcome lands in this component, and closing
          early would hand back a bar that looks idle while posts are still
          being changed. */}
      <AlertDialog
        open={pending !== null}
        onOpenChange={(next) => {
          if (!next && !busy) {
            setPending(null);
            setRunError(null);
          }
        }}
      >
        <AlertDialogContent
          onCloseAutoFocus={returnFocus.onCloseAutoFocus}
          onOpenAutoFocus={(e) => {
            returnFocus.onOpenAutoFocus();
            // A tag action cannot go ahead without a name, so the field is
            // where the work starts. Everything else starts on Cancel.
            if (shown === 'addTag' || shown === 'removeTag') {
              e.preventDefault();
              tagInput.current?.focus();
            }
          }}
          className='block w-full rounded-[8px] border border-border bg-card p-4 ring-0 data-[size=default]:max-w-[min(480px,calc(100%-3rem))] data-[size=default]:sm:max-w-[min(480px,calc(100%-3rem))]'
        >
          {shown && (
            <>
              <AlertDialogTitle className='text-ui font-semibold text-foreground'>{LABEL[shown]}</AlertDialogTitle>
              <AlertDialogDescription className='mt-2 text-meta leading-[1.6] text-wrap text-body md:text-wrap'>
                {describe(shown, selected.length, tag.trim() || '…')}
              </AlertDialogDescription>

              {(shown === 'addTag' || shown === 'removeTag') && (
                <Input
                  ref={tagInput}
                  value={tag}
                  onChange={(e) => setTag(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') void confirm();
                  }}
                  placeholder='Tag name'
                  aria-label='Tag name'
                  spellCheck={false}
                  className='mt-3 h-[30px] font-mono text-meta'
                />
              )}

              {REACHES_READERS[shown] && (
                <p className='mt-2 text-label leading-[1.6] text-warning'>
                  This one reaches Cloudflare, once per post.
                </p>
              )}

              {runError && (
                <p role='alert' className='mt-3 text-meta leading-[1.6] text-pretty text-destructive'>
                  Could not run this: {runError}
                </p>
              )}

              <div className='mt-4 flex items-center justify-end gap-2'>
                <AlertDialogCancel variant='ghost' size='sm' disabled={busy} className='h-[30px] text-meta'>
                  Cancel
                </AlertDialogCancel>
                {/* A plain button, not `AlertDialogAction`: that one closes the
                    dialog on click, and this has to stay up until the run is
                    done. */}
                <Button
                  size='sm'
                  variant={shown === 'trash' ? 'destructive' : 'default'}
                  onClick={() => void confirm()}
                  disabled={busy || ((shown === 'addTag' || shown === 'removeTag') && tag.trim() === '')}
                  className='h-[30px] gap-1.5 text-meta font-semibold'
                >
                  {busy && <Loader2 size={12} strokeWidth={2} className='animate-spin' />}
                  {busy ? 'Working…' : LABEL[shown]}
                </Button>
              </div>
            </>
          )}
        </AlertDialogContent>
      </AlertDialog>

      {/* The result, read out once the dialog that started the run has closed —
          without it, a screen reader user hears focus come back and nothing
          about what was done. A live region is only announced reliably when
          it is already in the page as its text arrives, so this one exists
          for as long as the bar does and is filled in when a run finishes.
          It goes to <body> because an empty element here would still take a
          gap in the page's spacing. */}
      {typeof document !== 'undefined' &&
        createPortal(
          <div role='status' className='sr-only'>
            {outcome && summarize(outcome)}
          </div>,
          document.body,
        )}

      {/* What happened, per post where it did not work. Nothing is rolled back:
          the posts that succeeded stay done, and the ones that did not are
          named rather than folded into a count. */}
      {outcome && (
        <div className='space-y-2'>
          <div className='flex items-start gap-2 rounded-[6px] border border-border bg-muted/50 px-3 py-2'>
            <p className='flex-1 text-meta text-body'>{summarize(outcome)}</p>
            <button
              type='button'
              onClick={() => setOutcome(null)}
              aria-label='Dismiss'
              className='shrink-0 rounded-[4px] p-0.5 text-muted-foreground transition-colors hover:bg-muted'
            >
              <X size={12} strokeWidth={2} />
            </button>
          </div>

          {outcome.failed.map((f) => (
            <p key={f.id} className='px-3 text-label leading-[1.6] text-destructive'>
              <span className='font-medium'>{f.title}</span>: {f.message}
            </p>
          ))}

          {outcome.skipped.filter((p) => p.reason === 'body_not_cached').length > 0 && (
            <p className='px-3 text-label leading-[1.6] text-warning'>
              Left unchanged because their text is not on this machine, so the edit could not be recorded and the next
              refresh would undo it:{' '}
              {outcome.skipped
                .filter((p) => p.reason === 'body_not_cached')
                .map((p, i) => (
                  <span key={p.id}>
                    {i > 0 && ', '}
                    <span className='font-medium'>{p.title}</span>
                  </span>
                ))}
              . Open them once to bring the text down, then try again.
            </p>
          )}

          {outcome.skipped.filter((p) => p.reason === 'body_stale').length > 0 && (
            <p className='px-3 text-label leading-[1.6] text-warning'>
              Left unchanged because the copy of their text here is behind the cloud&apos;s. Recording an edit against
              it would put this machine&apos;s older text forward as the newer one:{' '}
              {outcome.skipped
                .filter((p) => p.reason === 'body_stale')
                .map((p, i) => (
                  <span key={p.id}>
                    {i > 0 && ', '}
                    <span className='font-medium'>{p.title}</span>
                  </span>
                ))}
              . Open them once to bring the current text down, then try again.
            </p>
          )}
        </div>
      )}
    </>
  );
}
