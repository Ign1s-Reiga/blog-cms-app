/// `edited` is a published post carrying local changes readers have not been
/// served yet — live, but not *this* version. It is deliberately its own status
/// rather than a shade of `published`, because the two call for different
/// actions: one is finished, the other is waiting to be published.
export type PostStatus = 'published' | 'draft' | 'failed' | 'edited';

const DOT_COLORS: Record<PostStatus, string> = {
  published: 'bg-emerald-500',
  failed: 'bg-red-500',
  edited: 'bg-sky-500',
  draft: 'bg-amber-400',
};

export function StatusDot({ status }: { status: PostStatus }) {
  return (
    <span className={['inline-block w-[6px] h-[6px] rounded-full shrink-0', DOT_COLORS[status]].join(' ')} />
  );
}
