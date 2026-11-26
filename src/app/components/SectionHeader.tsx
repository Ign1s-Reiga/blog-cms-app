import { Separator } from '@/components/ui/separator';

export function SectionHeader({ children, action }: { children: React.ReactNode; action?: React.ReactNode }) {
  return (
    <div className='flex items-center gap-3 mb-5'>
      <h2 className='text-label font-bold text-muted-foreground uppercase tracking-[0.12em] shrink-0'>{children}</h2>
      <Separator className='flex-1 bg-border/60' />
      {action}
    </div>
  );
}
