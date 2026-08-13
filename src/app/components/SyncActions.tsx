'use client';

import { useCallback, useEffect, useRef, useState } from 'react';
import { CloudDownload, CloudUpload, type LucideIcon } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { cn } from '@/lib/utils';
import { pullFromCloud, pushToCloud } from '@/lib/sync';

/// A single header sync control. `runOnMount` runs the action once when the app
/// shell mounts (used for the launch-time pull). No-ops outside the Tauri
/// runtime — the underlying sync helpers guard that.
function SyncAction({
  icon: Icon,
  label,
  run,
  runOnMount = false,
}: {
  icon: LucideIcon;
  label: string;
  run: () => Promise<void>;
  runOnMount?: boolean;
}) {
  const [busy, setBusy] = useState(false);
  const guard = useRef(false);

  const trigger = useCallback(async () => {
    if (guard.current) return;
    guard.current = true;
    setBusy(true);
    try {
      await run();
    } catch (err) {
      console.error(`${label} failed:`, err);
    } finally {
      guard.current = false;
      setBusy(false);
    }
  }, [run, label]);

  useEffect(() => {
    if (runOnMount) void trigger();
  }, [runOnMount, trigger]);

  return (
    <Button
      variant='ghost'
      size='icon'
      aria-label={label}
      title={label}
      onClick={trigger}
      disabled={busy}
      className='size-[30px] rounded-[6px] text-zinc-400 dark:text-zinc-500 hover:text-zinc-700 dark:hover:text-zinc-300'
    >
      <Icon size={15} strokeWidth={1.8} className={cn(busy && 'animate-pulse')} />
    </Button>
  );
}

/// Header cloud-sync cluster: pull (D1 → local) and push (local → D1). The pull
/// also runs once on launch since this mounts with the app shell.
export function SyncActions() {
  return (
    <>
      <SyncAction icon={CloudDownload} label='Pull from cloud' run={pullFromCloud} runOnMount />
      <SyncAction icon={CloudUpload} label='Push to cloud' run={pushToCloud} />
    </>
  );
}
