import { Breadcrumb } from '@/components/Breadcrumb';
import { SidebarToggleBtn } from '@/components/SidebarToggleBtn';
import { SyncActions } from '@/components/SyncActions';
import { ThemeToggle } from '@/components/ThemeToggle';
import { cn } from '@/lib/utils';

export function Header() {
  return (
    <header
      className={cn(
        'relative h-[52px] shrink-0 flex items-center px-3',
        'bg-sidebar',
        'border-b border-border',
        'shadow-[0_1px_0_0_rgba(0,0,0,0.03)] dark:shadow-none',
      )}
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

        {/* Theme toggle */}
        <ThemeToggle />
      </div>
    </header>
  );
}
