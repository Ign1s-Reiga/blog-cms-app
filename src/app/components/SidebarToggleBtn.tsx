"use client";

import { PanelLeft } from "lucide-react";
import { useSidebar } from "./SidebarProvider";

export function SidebarToggleBtn() {
  const { toggle } = useSidebar();
  return (
    <button
      onClick={toggle}
      aria-label="Toggle sidebar"
      className={[
        "w-[30px] h-[30px] flex items-center justify-center rounded-[6px]",
        "text-zinc-400 dark:text-zinc-500",
        "hover:bg-zinc-100 dark:hover:bg-white/[0.06]",
        "hover:text-zinc-700 dark:hover:text-zinc-300",
        "active:scale-[0.92] active:bg-zinc-200 dark:active:bg-white/[0.1] active:transition-none",
        "transition-[background-color,color,transform] duration-100",
      ].join(" ")}
    >
      <PanelLeft size={15} strokeWidth={1.8} />
    </button>
  );
}
