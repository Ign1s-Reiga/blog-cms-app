import { Bell } from "lucide-react";
import { Breadcrumb } from "./Breadcrumb";
import { SidebarToggleBtn } from "./SidebarToggleBtn";
import { SyncButton } from "./SyncButton";
import { ThemeToggle } from "./ThemeToggle";
import { Button } from "@/components/ui/button";
import { Avatar, AvatarFallback } from "@/components/ui/avatar";
import { Separator } from "@/components/ui/separator";

export function Header() {
  return (
    <header
      className={[
        "relative h-[52px] shrink-0 flex items-center px-3",
        "bg-white dark:bg-[#111111]",
        "border-b border-zinc-200 dark:border-white/[0.06]",
        "shadow-[0_1px_0_0_rgba(0,0,0,0.03)] dark:shadow-none",
      ].join(" ")}
    >
      {/* ── Left: sidebar toggle + breadcrumb ─────────────────────────── */}
      <div className="flex items-center gap-1.5 shrink-0">
        <SidebarToggleBtn />
        <Breadcrumb />
      </div>

      {/* ── Right: action cluster ─────────────────────────────────────── */}
      <div className="flex items-center gap-0.5 ml-auto shrink-0">
        {/* Sync to cloud */}
        <SyncButton />

        {/* Notification bell */}
        <div className="relative">
          <Button
            variant="ghost"
            size="icon"
            aria-label="Notifications"
            className="size-[30px] rounded-[6px] text-zinc-400 dark:text-zinc-500 hover:text-zinc-700 dark:hover:text-zinc-300"
          >
            <Bell size={15} strokeWidth={1.8} />
          </Button>
          <span
            aria-label="Unread notifications"
            className="absolute top-[5px] right-[5px] w-[6px] h-[6px] rounded-full bg-indigo-500 dark:bg-indigo-400 ring-[1.5px] ring-white dark:ring-[#111111] pointer-events-none"
          />
        </div>

        {/* Theme toggle */}
        <ThemeToggle />

        {/* Vertical rule */}
        <Separator
          orientation="vertical"
          className="mx-[6px] h-[18px] bg-zinc-200 dark:bg-white/[0.08]"
        />

        {/* User avatar */}
        <button
          aria-label="User menu"
          className="relative rounded-full ring-[1.5px] ring-transparent hover:ring-indigo-400/50 dark:hover:ring-indigo-400/40 hover:ring-offset-[2px] hover:ring-offset-white dark:hover:ring-offset-[#111111] active:scale-[0.9] active:transition-none transition-all duration-150"
        >
          <Avatar className="size-[26px] after:hidden">
            <AvatarFallback className="text-[11px] font-bold text-white bg-gradient-to-br from-violet-400 to-indigo-600">
              A
            </AvatarFallback>
          </Avatar>
        </button>
      </div>
    </header>
  );
}
