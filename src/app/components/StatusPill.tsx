import { StatusDot, type PostStatus } from '@/components/StatusDot';
import { Badge } from '@/components/ui/badge';
import { cn } from '@/lib/utils';

/// Label and colour per status. `edited` says what is *waiting* rather than what
/// the post is, because that is the part the reader of this list has to act on:
/// the post is live, and this version of it is not.
const PILLS: Record<PostStatus, { label: string; title: string; className: string }> = {
  published: {
    label: 'Published',
    title: 'Live, and this is the version readers see',
    className: 'bg-success/10 text-success border-success/30',
  },
  edited: {
    label: 'Unpublished edits',
    title: 'Live, but readers still see the previous version — publish to update it',
    className:
      'bg-sky-50 text-sky-700 dark:bg-sky-500/[0.12] dark:text-sky-400 border-sky-200/80 dark:border-sky-500/20',
  },
  conflict: {
    label: 'Conflict',
    title: 'This post changed here and in the cloud — neither can be applied without losing the other',
    className:
      'bg-orange-50 text-orange-700 dark:bg-orange-500/[0.12] dark:text-orange-400 border-orange-200/80 dark:border-orange-500/20',
  },
  behind: {
    label: 'Cloud is newer',
    title: 'The cloud has a newer version and this machine has no pending changes',
    className:
      'bg-violet-50 text-violet-700 dark:bg-violet-500/[0.12] dark:text-violet-400 border-violet-200/80 dark:border-violet-500/20',
  },
  scheduled: {
    label: 'Scheduled',
    title: 'Already uploaded and waiting for its time — Cloudflare publishes it, whether this app is running or not',
    className:
      'bg-indigo-50 text-indigo-700 dark:bg-indigo-500/[0.12] dark:text-indigo-400 border-indigo-200/80 dark:border-indigo-500/20',
  },
  overdue: {
    label: 'Overdue',
    title: 'Its time has passed and it is still not live — the scheduler Worker has not run it',
    className:
      'bg-orange-50 text-orange-700 dark:bg-orange-500/[0.12] dark:text-orange-400 border-orange-200/80 dark:border-orange-500/20',
  },
  failed: {
    label: 'Sync failed',
    title: 'The last publish did not reach the cloud, so these edits are not live',
    className: 'bg-destructive/10 text-destructive border-destructive/30',
  },
  draft: {
    label: 'Draft',
    title: 'Local only — nothing has been published',
    className: 'bg-warning/10 text-warning border-warning/30',
  },
};

export function StatusPill({ status }: { status: PostStatus }) {
  const { label, title, className } = PILLS[status];
  return (
    <Badge
      variant='outline'
      title={title}
      className={cn('gap-[5px] px-[7px] py-[3px] rounded-full text-label font-semibold', className)}
    >
      <StatusDot status={status} />
      {label}
    </Badge>
  );
}
