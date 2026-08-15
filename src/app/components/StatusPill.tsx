import { StatusDot, type PostStatus } from '@/components/StatusDot';
import { Badge } from '@/components/ui/badge';

/// Label and colour per status. `edited` says what is *waiting* rather than what
/// the post is, because that is the part the reader of this list has to act on:
/// the post is live, and this version of it is not.
const PILLS: Record<PostStatus, { label: string; title: string; className: string }> = {
  published: {
    label: 'Published',
    title: 'Live, and this is the version readers see',
    className:
      'bg-emerald-50 text-emerald-700 dark:bg-emerald-500/[0.12] dark:text-emerald-400 border-emerald-200/80 dark:border-emerald-500/20',
  },
  edited: {
    label: 'Unpublished edits',
    title: 'Live, but readers still see the previous version — publish to update it',
    className:
      'bg-sky-50 text-sky-700 dark:bg-sky-500/[0.12] dark:text-sky-400 border-sky-200/80 dark:border-sky-500/20',
  },
  failed: {
    label: 'Sync failed',
    title: 'The last publish did not reach the cloud, so these edits are not live',
    className:
      'bg-red-50 text-red-700 dark:bg-red-500/[0.12] dark:text-red-400 border-red-200/80 dark:border-red-500/20',
  },
  draft: {
    label: 'Draft',
    title: 'Local only — nothing has been published',
    className:
      'bg-amber-50 text-amber-700 dark:bg-amber-500/[0.12] dark:text-amber-400 border-amber-200/80 dark:border-amber-500/20',
  },
};

export function StatusPill({ status }: { status: PostStatus }) {
  const { label, title, className } = PILLS[status];
  return (
    <Badge
      variant='outline'
      title={title}
      className={['gap-[5px] px-[7px] py-[3px] rounded-full text-[11px] font-semibold', className].join(' ')}
    >
      <StatusDot status={status} />
      {label}
    </Badge>
  );
}
