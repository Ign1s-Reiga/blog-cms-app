export type PostStatus = 'published' | 'draft' | 'failed';

export function StatusDot({ status }: { status: PostStatus }) {
  const color = status === 'published' ? 'bg-emerald-500' : status === 'failed' ? 'bg-red-500' : 'bg-amber-400';
  return <span className={['inline-block w-[6px] h-[6px] rounded-full shrink-0', color].join(' ')} />;
}
