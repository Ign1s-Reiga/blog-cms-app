/// `edited` is a published post carrying local changes readers have not been
/// served yet — live, but not *this* version. It is deliberately its own status
/// rather than a shade of `published`, because the two call for different
/// actions: one is finished, the other is waiting to be published.
///
/// `scheduled` and `overdue` are the two halves of a pending publication: one is
/// waiting for its time, the other is past it and still waiting — which means
/// the Worker that was supposed to run it has not.
import { cn } from '@/lib/utils';

export type PostStatus = 'published' | 'draft' | 'failed' | 'edited' | 'behind' | 'conflict' | 'scheduled' | 'overdue';

const DOT_COLORS: Record<PostStatus, string> = {
  published: 'bg-success',
  failed: 'bg-destructive',
  conflict: 'bg-orange-500',
  edited: 'bg-sky-500',
  behind: 'bg-violet-500',
  scheduled: 'bg-indigo-500',
  overdue: 'bg-orange-500',
  draft: 'bg-warning',
};

/// Colour only, so never the only statement of a status: every caller writes
/// the status out beside it (`StatusPill`'s label, the dashboard's meta line).
/// That makes the dot decorative, and it is hidden from screen readers so they
/// do not meet an empty, unnamed element on every row.
export function StatusDot({ status }: { status: PostStatus }) {
  return (
    <span aria-hidden='true' className={cn('inline-block w-[6px] h-[6px] rounded-full shrink-0', DOT_COLORS[status])} />
  );
}
