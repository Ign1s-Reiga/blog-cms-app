'use client';

import {
  ArrowUpCircle,
  BarChart2,
  FileText,
  Image,
  Layers,
  LayoutDashboard,
  Plus,
  Settings,
  Tags,
  Zap,
} from 'lucide-react';
import Link from 'next/link';
import { usePathname, useRouter } from 'next/navigation';
import { useCallback, useEffect, useState } from 'react';
import { useSidebar } from '@/components/SidebarProvider';
import { Button } from '@/components/ui/button';
import { Badge } from '@/components/ui/badge';
import { Separator } from '@/components/ui/separator';
import { onPostsRefreshed } from '@/lib/sync';
import { checkForUpdate, getCachedStatus, onUpdateStatusChanged } from '@/lib/updater';
import { cn } from '@/lib/utils';

// ─── Types ────────────────────────────────────────────────────────────────────

interface NavItem {
  href: string;
  label: string;
  Icon: React.ComponentType<{ size?: number; strokeWidth?: number; className?: string }>;
  badge?: number;
}

// ─── Data ─────────────────────────────────────────────────────────────────────

const NAV_MAIN: NavItem[] = [
  { href: '/', label: 'Dashboard', Icon: LayoutDashboard },
  { href: '/posts', label: 'Posts', Icon: FileText },
  { href: '/series', label: 'Series', Icon: Layers },
  { href: '/tags', label: 'Tags', Icon: Tags },
  { href: '/media', label: 'Media', Icon: Image },
];

const NAV_TOOLS: NavItem[] = [
  { href: '/analytics', label: 'Analytics', Icon: BarChart2 },
  { href: '/settings', label: 'Settings', Icon: Settings },
];

// ─── NavLink ──────────────────────────────────────────────────────────────────

function NavLink({ href, label, Icon, badge, active, collapsed }: NavItem & { active: boolean; collapsed: boolean }) {
  return (
    <Link
      href={href}
      // The rail and the weight say "you are here" only to someone who can see
      // them; this says it to a screen reader.
      aria-current={active ? 'page' : undefined}
      title={collapsed ? label : undefined}
      className={cn(
        'relative group w-full flex items-center h-[32px] rounded-[5px] select-none',
        collapsed ? 'justify-center' : 'px-2.5 gap-2.5',
        'transition-[background-color,color] duration-100 ease-out',
        'active:scale-[0.97] active:transition-none',
        active ? 'bg-muted text-foreground' : 'text-muted-foreground hover:bg-muted/50 hover:text-foreground',
      )}
    >
      {/* Active left-rail indicator */}
      <span
        aria-hidden
        className={cn(
          'absolute left-0 top-1/2 -translate-y-1/2 w-[2px] rounded-r-full',
          'bg-body',
          'transition-[height,opacity] duration-200',
          active ? 'h-[18px] opacity-100' : 'h-0 opacity-0',
        )}
      />

      {/* Icon */}
      <span
        className={cn(
          'shrink-0 transition-colors duration-100',
          active ? 'text-body' : 'text-muted-foreground group-hover:text-body',
        )}
      >
        <Icon size={14} strokeWidth={active ? 2.2 : 1.8} />
      </span>

      {/* Label + badge */}
      {!collapsed && (
        <>
          <span
            className={cn('flex-1 text-left text-ui leading-none truncate', active ? 'font-semibold' : 'font-medium')}
          >
            {label}
          </span>
          {badge !== undefined && (
            <Badge
              className={cn(
                'min-w-[18px] h-[18px] px-1 rounded-full text-label font-bold tabular-nums border-transparent',
                active ? 'bg-accent text-body' : 'bg-muted text-muted-foreground',
              )}
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
  onClick,
}: {
  icon: React.ReactNode;
  label: string;
  end?: React.ReactNode;
  onClick?: () => void;
}) {
  return (
    <button
      onClick={onClick}
      className='group w-full flex items-center gap-2 h-[28px] px-2 rounded-[5px] transition-[background-color] duration-100 hover:bg-muted/50'
    >
      <span className='shrink-0 text-muted-foreground group-hover:text-muted-foreground transition-colors duration-100'>
        {icon}
      </span>
      <span className='flex-1 text-left text-meta font-medium text-muted-foreground group-hover:text-body transition-colors duration-100 truncate'>
        {label}
      </span>
      {end}
    </button>
  );
}

// ─── Sidebar ──────────────────────────────────────────────────────────────────

export function Sidebar() {
  const pathname = usePathname();
  const router = useRouter();
  const { collapsed } = useSidebar();

  // Live post count for the Posts badge, from the local cache.
  // `null` while unknown; re-read on navigation and after a cloud refresh so it
  // tracks adds/deletes. An empty database or plain browser leaves it unset, so
  // the badge simply doesn't render.
  const [postCount, setPostCount] = useState<number | null>(null);

  const loadCount = useCallback(async () => {
    const { invoke, isTauri } = await import('@tauri-apps/api/core');
    if (!isTauri()) return;
    try {
      const rows = await invoke<unknown[]>('list_posts');
      setPostCount(Array.isArray(rows) ? rows.length : 0);
    } catch {
      setPostCount(0);
    }
  }, []);

  useEffect(() => {
    void loadCount();
  }, [loadCount, pathname]);

  // Re-read the count after a cloud refresh.
  useEffect(() => onPostsRefreshed(() => void loadCount()), [loadCount]);

  // The launch-time update check. The helper caches per session, so mounting
  // this (or opening Settings) later replays the same answer instead of hitting
  // GitHub again. `null` keeps the notice hidden — including in the browser.
  const [newVersion, setNewVersion] = useState<string | null>(() => {
    const cached = getCachedStatus();
    return cached?.available ? cached.version : null;
  });

  useEffect(() => {
    const show = (s: { available: boolean; version: string | null }) => setNewVersion(s.available ? s.version : null);
    // Listen before checking so a fast resolve can't slip past the subscription,
    // then apply the result directly (it may be a replay of the cached status).
    const unlisten = onUpdateStatusChanged(show);
    void checkForUpdate()
      .then((s) => s && show(s))
      .catch(() => {}); // offline / no release yet — stay quiet
    return unlisten;
  }, []);

  const isActive = (href: string) =>
    href === '/' ? pathname === '/' : pathname === href || pathname.startsWith(href + '/');

  return (
    <aside
      style={{ width: collapsed ? 56 : 220 }}
      className={cn(
        'relative shrink-0 flex flex-col h-full overflow-hidden',
        'bg-sidebar',
        'border-r border-border',
        'transition-[width] duration-[220ms] ease-[cubic-bezier(0.25,0.46,0.45,0.94)] motion-reduce:transition-none',
      )}
    >
      {/* ── Brand ──────────────────────────────────────────────────────── */}
      <div
        className={cn(
          'h-[52px] shrink-0 flex items-center',
          'border-b border-border/60',
          collapsed ? 'justify-center' : 'px-3 gap-2.5',
        )}
      >
        <div className='w-[26px] h-[26px] rounded-[6px] bg-primary flex items-center justify-center shrink-0'>
          <Zap size={13} strokeWidth={2.5} className='text-primary-foreground' />
        </div>

        {!collapsed && (
          <div className='flex-1 min-w-0'>
            <p className='text-ui font-semibold text-foreground leading-none truncate'>My Blog</p>
            <p className='text-label text-muted-foreground mt-[3px] truncate font-mono tracking-tight'>blog-cms-app</p>
          </div>
        )}
      </div>

      {/* ── New Post CTA ───────────────────────────────────────────────── */}
      <div className={cn('py-2.5', collapsed ? 'px-2' : 'px-2.5')}>
        <Button
          asChild
          className={cn(
            'w-full h-auto rounded-[6px] text-ui font-semibold leading-none',
            'shadow-[0_1px_2px_rgba(0,0,0,0.1)]',
            'hover:shadow-[0_2px_8px_rgba(0,0,0,0.18)] dark:hover:shadow-[0_2px_8px_rgba(0,0,0,0.5)]',
            collapsed ? 'p-2' : 'gap-[6px] py-[7px] px-3',
          )}
        >
          <Link href='/posts/new' title={collapsed ? 'New Post' : undefined}>
            <Plus size={13} strokeWidth={2.5} />
            {!collapsed && 'New Post'}
          </Link>
        </Button>
      </div>

      {/* ── Navigation ─────────────────────────────────────────────────── */}
      <nav className='flex-1 overflow-y-auto px-2 space-y-px'>
        {NAV_MAIN.map(({ href, label, Icon, badge }) => (
          <NavLink
            key={href}
            href={href}
            label={label}
            Icon={Icon}
            badge={href === '/posts' ? (postCount && postCount > 0 ? postCount : undefined) : badge}
            active={isActive(href)}
            collapsed={collapsed}
          />
        ))}

        <Separator className='my-[6px] bg-border' />

        {!collapsed && (
          <p className='px-2.5 pt-[2px] pb-[4px] text-label font-bold uppercase tracking-[0.12em] text-muted-foreground'>
            Tools
          </p>
        )}

        {NAV_TOOLS.map(({ href, label, Icon }) => (
          <NavLink key={href} href={href} label={label} Icon={Icon} active={isActive(href)} collapsed={collapsed} />
        ))}
      </nav>

      {/* ── Update notice ──────────────────────────────────────────────── */}
      {/* The only footer entry that does something: it appears when a release is
          waiting and takes you to Settings to install it. */}
      {!collapsed && newVersion && (
        <div className='shrink-0 border-t border-border/60 px-2 pt-2 pb-3'>
          <UtilLink
            icon={<ArrowUpCircle size={13} strokeWidth={1.8} />}
            label='Update available'
            onClick={() => router.push('/settings')}
            end={
              <Badge className='shrink-0 h-[16px] px-1.5 rounded-full border-transparent bg-success/10 text-label font-bold tabular-nums text-success'>
                v{newVersion}
              </Badge>
            }
          />
        </div>
      )}
    </aside>
  );
}
