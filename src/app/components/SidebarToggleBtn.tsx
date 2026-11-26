'use client';

import { PanelLeft } from 'lucide-react';
import { useSidebar } from '@/components/SidebarProvider';
import { Button } from '@/components/ui/button';

export function SidebarToggleBtn() {
  const { collapsed, toggle } = useSidebar();
  return (
    <Button
      variant='ghost'
      size='icon'
      onClick={toggle}
      aria-label='Toggle sidebar'
      aria-expanded={!collapsed}
      // The ghost variant paints aria-expanded as pressed, which suits a menu
      // trigger, not a panel that is open by default.
      className='size-[30px] rounded-[6px] text-muted-foreground hover:text-body aria-expanded:bg-transparent aria-expanded:text-muted-foreground aria-expanded:hover:bg-muted aria-expanded:hover:text-body'
    >
      <PanelLeft size={15} strokeWidth={1.8} />
    </Button>
  );
}
