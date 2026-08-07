"use client";

import { PanelLeft } from "lucide-react";
import { useSidebar } from "@/components/SidebarProvider";
import { Button } from "@/components/ui/button";

export function SidebarToggleBtn() {
  const { toggle } = useSidebar();
  return (
    <Button
      variant="ghost"
      size="icon"
      onClick={toggle}
      aria-label="Toggle sidebar"
      className="size-[30px] rounded-[6px] text-zinc-400 dark:text-zinc-500 hover:text-zinc-700 dark:hover:text-zinc-300"
    >
      <PanelLeft size={15} strokeWidth={1.8} />
    </Button>
  );
}
