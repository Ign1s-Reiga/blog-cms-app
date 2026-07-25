"use client";

import {
  BarChart2,
  BookOpen,
  ChevronDown,
  ChevronLeft,
  ExternalLink,
  FileText,
  Image,
  Keyboard,
  LayoutDashboard,
  Plus,
  Settings,
  Zap,
} from "lucide-react";
import Link from "next/link";
import { usePathname } from "next/navigation";
import { useSidebar } from "./SidebarProvider";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Avatar, AvatarFallback } from "@/components/ui/avatar";
import { Separator } from "@/components/ui/separator";

// ─── Types ────────────────────────────────────────────────────────────────────

interface NavItem {
  href: string;
  label: string;
  Icon: React.ComponentType<{ size?: number; strokeWidth?: number; className?: string }>;
  badge?: number;
}

// ─── Data ─────────────────────────────────────────────────────────────────────

const NAV_MAIN: NavItem[] = [
  { href: "/", label: "Dashboard", Icon: LayoutDashboard },
  { href: "/posts",     label: "Posts",     Icon: FileText, badge: 2 },
  { href: "/media",     label: "Media",     Icon: Image },
];

const NAV_TOOLS: NavItem[] = [
  { href: "/analytics", label: "Analytics", Icon: BarChart2 },
  { href: "/settings",  label: "Settings",  Icon: Settings },
];

// ─── NavLink ──────────────────────────────────────────────────────────────────

function NavLink({
  href,
  label,
  Icon,
  badge,
  active,
  collapsed,
}: NavItem & { active: boolean; collapsed: boolean }) {
  return (
    <Link
      href={href}
      title={collapsed ? label : undefined}
      className={[
        "relative group w-full flex items-center h-[32px] rounded-[5px] select-none",
        collapsed ? "justify-center" : "px-2.5 gap-2.5",
        "transition-[background-color,color] duration-100 ease-out",
        "active:scale-[0.97] active:transition-none",
        active
          ? "bg-zinc-100 dark:bg-white/[0.08] text-zinc-900 dark:text-zinc-50"
          : "text-zinc-500 dark:text-zinc-500 hover:bg-zinc-50 dark:hover:bg-white/[0.04] hover:text-zinc-800 dark:hover:text-zinc-300",
      ].join(" ")}
    >
      {/* Active left-rail indicator */}
      <span
        aria-hidden
        className={[
          "absolute left-0 top-1/2 -translate-y-1/2 w-[2px] rounded-r-full",
          "bg-zinc-700 dark:bg-zinc-200",
          "transition-[height,opacity] duration-200",
          active ? "h-[18px] opacity-100" : "h-0 opacity-0",
        ].join(" ")}
      />

      {/* Icon */}
      <span
        className={[
          "shrink-0 transition-colors duration-100",
          active
            ? "text-zinc-700 dark:text-zinc-200"
            : "text-zinc-400 dark:text-zinc-600 group-hover:text-zinc-600 dark:group-hover:text-zinc-400",
        ].join(" ")}
      >
        <Icon size={14} strokeWidth={active ? 2.2 : 1.8} />
      </span>

      {/* Label + badge */}
      {!collapsed && (
        <>
          <span
            className={[
              "flex-1 text-left text-[13px] leading-none truncate",
              active ? "font-semibold" : "font-medium",
            ].join(" ")}
          >
            {label}
          </span>
          {badge !== undefined && (
            <Badge
              className={[
                "min-w-[18px] h-[18px] px-1 rounded-full text-[10px] font-bold tabular-nums border-transparent",
                active
                  ? "bg-zinc-200 dark:bg-white/[0.14] text-zinc-600 dark:text-zinc-300"
                  : "bg-zinc-100 dark:bg-white/[0.06] text-zinc-400 dark:text-zinc-500",
              ].join(" ")}
            >
              {badge}
            </Badge>
          )}
        </>
      )}
    </Link>
  );
}

// ─── UtilLink ─────────────────────────────────────────────────────────────────

function UtilLink({
  icon,
  label,
  end,
}: {
  icon: React.ReactNode;
  label: string;
  end?: React.ReactNode;
}) {
  return (
    <button className="group w-full flex items-center gap-2 h-[28px] px-2 rounded-[5px] transition-[background-color] duration-100 hover:bg-zinc-50 dark:hover:bg-white/[0.04]">
      <span className="shrink-0 text-zinc-400 dark:text-zinc-600 group-hover:text-zinc-500 dark:group-hover:text-zinc-500 transition-colors duration-100">
        {icon}
      </span>
      <span className="flex-1 text-left text-[12px] font-medium text-zinc-500 dark:text-zinc-500 group-hover:text-zinc-700 dark:group-hover:text-zinc-300 transition-colors duration-100 truncate">
        {label}
      </span>
      {end}
    </button>
  );
}

// ─── Sidebar ──────────────────────────────────────────────────────────────────

export function Sidebar() {
  const pathname = usePathname();
  const { collapsed, toggle } = useSidebar();

  const isActive = (href: string) =>
    href === "/" ? pathname === "/" : pathname === href || pathname.startsWith(href + "/");

  return (
    <aside
      style={{ width: collapsed ? 56 : 220 }}
      className={[
        "relative shrink-0 flex flex-col h-full overflow-hidden",
        "bg-white dark:bg-[#111111]",
        "border-r border-zinc-200 dark:border-white/[0.06]",
        "transition-[width] duration-[220ms] ease-[cubic-bezier(0.25,0.46,0.45,0.94)]",
      ].join(" ")}
    >
      {/* ── Workspace switcher ─────────────────────────────────────────── */}
      <div
        className={[
          "h-[52px] shrink-0 flex items-center",
          "border-b border-zinc-100 dark:border-white/[0.05]",
          collapsed ? "justify-center" : "px-3 gap-2.5",
        ].join(" ")}
      >
        <div className="w-[26px] h-[26px] rounded-[6px] bg-zinc-900 dark:bg-zinc-100 flex items-center justify-center shrink-0">
          <Zap size={13} strokeWidth={2.5} className="text-white dark:text-zinc-900" />
        </div>

        {!collapsed && (
          <>
            <div className="flex-1 min-w-0">
              <p className="text-[13px] font-semibold text-zinc-900 dark:text-zinc-100 leading-none truncate">
                My Blog
              </p>
              <p className="text-[11px] text-zinc-400 dark:text-zinc-600 mt-[3px] truncate font-mono tracking-tight">
                blog-cms-app
              </p>
            </div>
            <Button
              variant="ghost"
              size="icon"
              className="size-5 rounded-[4px] text-zinc-400 dark:text-zinc-600 hover:text-zinc-600 dark:hover:text-zinc-400 shrink-0"
            >
              <ChevronDown size={12} strokeWidth={2} />
            </Button>
          </>
        )}
      </div>

      {/* ── New Post CTA ───────────────────────────────────────────────── */}
      <div className={["py-2.5", collapsed ? "px-2" : "px-2.5"].join(" ")}>
        <Button
          asChild
          className={[
            "w-full h-auto rounded-[6px] text-[13px] font-semibold leading-none",
            "shadow-[0_1px_2px_rgba(0,0,0,0.1)]",
            "hover:shadow-[0_2px_8px_rgba(0,0,0,0.18)] dark:hover:shadow-[0_2px_8px_rgba(0,0,0,0.5)]",
            collapsed ? "p-2" : "gap-[6px] py-[7px] px-3",
          ].join(" ")}
        >
          <Link href="/posts/new" title={collapsed ? "New Post" : undefined}>
            <Plus size={13} strokeWidth={2.5} />
            {!collapsed && "New Post"}
          </Link>
        </Button>
      </div>

      {/* ── Navigation ─────────────────────────────────────────────────── */}
      <nav className="flex-1 overflow-y-auto px-2 space-y-px">
        {NAV_MAIN.map(({ href, label, Icon, badge }) => (
          <NavLink
            key={href}
            href={href}
            label={label}
            Icon={Icon}
            badge={badge}
            active={isActive(href)}
            collapsed={collapsed}
          />
        ))}

        <Separator className="my-[6px] bg-zinc-100 dark:bg-white/[0.04]" />

        {!collapsed && (
          <p className="px-2.5 pt-[2px] pb-[4px] text-[10px] font-bold uppercase tracking-[0.12em] text-zinc-400 dark:text-zinc-600">
            Tools
          </p>
        )}

        {NAV_TOOLS.map(({ href, label, Icon }) => (
          <NavLink
            key={href}
            href={href}
            label={label}
            Icon={Icon}
            active={isActive(href)}
            collapsed={collapsed}
          />
        ))}
      </nav>

      {/* ── Bottom utilities ───────────────────────────────────────────── */}
      <div
        className={[
          "shrink-0 border-t border-zinc-100 dark:border-white/[0.05] pt-2 pb-3",
          collapsed ? "px-2" : "px-2",
        ].join(" ")}
      >
        {!collapsed && (
          <div className="space-y-px mb-2">
            <UtilLink
              icon={<BookOpen size={13} strokeWidth={1.8} />}
              label="Documentation"
              end={
                <ExternalLink
                  size={11}
                  strokeWidth={1.8}
                  className="shrink-0 text-zinc-300 dark:text-zinc-700 group-hover:text-zinc-400 dark:group-hover:text-zinc-500 transition-colors duration-100"
                />
              }
            />
            <UtilLink
              icon={
                <span className="w-[13px] h-[13px] flex items-center justify-center">
                  <span className="w-[6px] h-[6px] rounded-full bg-emerald-500" />
                </span>
              }
              label="System Status"
              end={
                <span className="shrink-0 text-[10px] font-bold text-emerald-600 dark:text-emerald-500 tracking-tight">
                  OK
                </span>
              }
            />
            <UtilLink
              icon={<Keyboard size={13} strokeWidth={1.8} />}
              label="Shortcuts"
              end={
                <kbd className="shrink-0 inline-flex items-center justify-center w-[18px] h-[18px] rounded-[3px] text-[11px] font-mono font-semibold bg-zinc-100 dark:bg-white/[0.06] text-zinc-400 dark:text-zinc-500 border border-zinc-200 dark:border-white/[0.08]">
                  ?
                </kbd>
              }
            />
          </div>
        )}

        <div
          className={[
            "flex items-center pt-2 border-t border-zinc-100 dark:border-white/[0.04]",
            collapsed ? "justify-center" : "gap-2.5 px-1",
          ].join(" ")}
        >
          <Avatar className="size-[26px] shrink-0 ring-[1.5px] ring-white dark:ring-[#111111] after:hidden">
            <AvatarFallback className="text-[11px] font-bold text-white bg-gradient-to-br from-violet-400 to-indigo-600">
              A
            </AvatarFallback>
            <span className="absolute -bottom-px -right-px z-10 w-[7px] h-[7px] rounded-full bg-emerald-500 ring-[1.5px] ring-white dark:ring-[#111111]" />
          </Avatar>
          {!collapsed && (
            <div className="flex-1 min-w-0">
              <p className="text-[12px] font-semibold text-zinc-700 dark:text-zinc-300 leading-none truncate">
                Admin
              </p>
              <p className="text-[11px] text-zinc-400 dark:text-zinc-600 mt-[3px] truncate font-mono tracking-tight">
                admin@blog.local
              </p>
            </div>
          )}
        </div>
      </div>

      <button
        onClick={toggle}
        aria-label={collapsed ? "Expand sidebar" : "Collapse sidebar"}
        className={[
          "absolute top-[66px] -right-[11px]",
          "w-[22px] h-[22px] rounded-full",
          "bg-white dark:bg-[#1c1c1c]",
          "border border-zinc-200 dark:border-white/[0.1]",
          "shadow-[0_1px_4px_rgba(0,0,0,0.08)]",
          "flex items-center justify-center",
          "text-zinc-400 dark:text-zinc-500",
          "hover:bg-zinc-50 dark:hover:bg-[#222]",
          "hover:text-zinc-600 dark:hover:text-zinc-300",
          "hover:border-zinc-300 dark:hover:border-white/[0.18]",
          "hover:shadow-[0_2px_8px_rgba(0,0,0,0.12)]",
          "active:scale-[0.9] active:shadow-none",
          "transition-all duration-150",
          "z-20",
        ].join(" ")}
      >
        <ChevronLeft
          size={11}
          strokeWidth={2.5}
          className={[
            "transition-transform duration-[220ms]",
            collapsed ? "rotate-180" : "",
          ].join(" ")}
        />
      </button>
    </aside>
  );
}
