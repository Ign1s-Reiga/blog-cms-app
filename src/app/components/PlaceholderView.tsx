import { BarChart2, FolderOpen, Settings } from 'lucide-react';

const ICONS = {
  folder: FolderOpen,
  analytics: BarChart2,
  settings: Settings,
} as const;

export function PlaceholderView({ icon, title, desc }: { icon: keyof typeof ICONS; title: string; desc: string }) {
  const Icon = ICONS[icon];
  return (
    <div className='flex flex-col items-center justify-center h-full min-h-[52vh] gap-4 select-none'>
      <div className='p-[18px] rounded-[14px] bg-white dark:bg-[#161616] border border-zinc-200 dark:border-white/[0.07] shadow-[0_1px_4px_rgba(0,0,0,0.04)]'>
        <Icon size={22} strokeWidth={1.5} className='text-zinc-300 dark:text-zinc-600' />
      </div>
      <div className='text-center'>
        <p className='text-[14px] font-semibold text-zinc-700 dark:text-zinc-300'>{title}</p>
        <p className='text-[12px] text-zinc-400 dark:text-zinc-600 mt-1.5 max-w-[260px] leading-relaxed'>{desc}</p>
      </div>
    </div>
  );
}
