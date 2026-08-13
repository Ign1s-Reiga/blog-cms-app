import { Bell } from 'lucide-react';
import { Breadcrumb } from '@/components/Breadcrumb';
import { SidebarToggleBtn } from '@/components/SidebarToggleBtn';
import { SyncActions } from '@/components/SyncActions';
import { ThemeToggle } from '@/components/ThemeToggle';
import { Button } from '@/components/ui/button';

export function Header() {
  return (
    <header
      className={[
        'relative h-[52px] shrink-0 flex items-center px-3',
        'bg-white dark:bg-[#111111]',
        'border-b border-zinc-200 dark:border-white/[0.06]',
        'shadow-[0_1px_0_0_rgba(0,0,0,0.03)] dark:shadow-none',
      ].join(' ')}
    >
      {/* ── Left: sidebar toggle + breadcrumb ─────────────────────────── */}
      <div className='flex items-center gap-1.5 shrink-0'>
        <SidebarToggleBtn />
        <Breadcrumb />
      </div>

      {/* ── Right: action cluster ─────────────────────────────────────── */}
      <div className='flex items-center gap-0.5 ml-auto shrink-0'>
        {/* Cloud sync: pull + push */}
        <SyncActions />

        {/* Notification bell */}
        <div className='relative'>
          <Button
            variant='ghost'
            size='icon'
            aria-label='Notifications'
            className='size-[30px] rounded-[6px] text-zinc-400 dark:text-zinc-500 hover:text-zinc-700 dark:hover:text-zinc-300'
          >
            <Bell size={15} strokeWidth={1.8} />
          </Button>
          <span
            aria-label='Unread notifications'
            className='absolute top-[5px] right-[5px] w-[6px] h-[6px] rounded-full bg-indigo-500 dark:bg-indigo-400 ring-[1.5px] ring-white dark:ring-[#111111] pointer-events-none'
          />
        </div>

        {/* Theme toggle */}
        <ThemeToggle />
      </div>
    </header>
  );
}
