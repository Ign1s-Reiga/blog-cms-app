'use client';

import { useCallback, useEffect, useRef, useState } from 'react';
import { CloudDownload, CloudUpload, type LucideIcon } from 'lucide-react';
import { toast } from 'sonner';
import { Button } from '@/components/ui/button';
import { cn } from '@/lib/utils';
import { pullFromCloud, pushToCloud } from '@/lib/sync';

/// A single header sync control. `runOnMount` runs the action once when the app
/// shell mounts (used for the launch-time pull). No-ops outside the Tauri
/// runtime — the underlying sync helpers guard that.
///
/// A click reports how it went either way, because neither direction
/// necessarily changes anything on screen. The launch-time run reports only a
/// failure — a pull that worked is what launching is expected to do — and not
/// even that while the app shell is mounted inert behind the login screen,
/// where a pull fails for want of credentials nobody has entered yet.
function SyncAction({
  icon: Icon,
  label,
  run,
  done,
  failed,
  runOnMount = false,
}: {
  icon: LucideIcon;
  label: string;
  run: () => Promise<void>;
  /// What a toast says when a click succeeded, and when it failed.
  done: string;
  failed: string;
  runOnMount?: boolean;
}) {
  const [busy, setBusy] = useState(false);
  const guard = useRef(false);
  const button = useRef<HTMLButtonElement>(null);

  const trigger = useCallback(
    async (clicked: boolean) => {
      if (guard.current) return;
      guard.current = true;
      setBusy(true);
      try {
        await run();
        if (clicked) toast.success(done);
      } catch (err) {
        console.error(`${label} failed:`, err);
        // `AuthGate` renders the app inert while it asks for credentials.
        const behindLogin = button.current?.closest('[inert]') != null;
        if (clicked || !behindLogin) toast.error(failed, { description: String(err) });
      } finally {
        guard.current = false;
        setBusy(false);
      }
    },
    [run, label, done, failed],
  );

  useEffect(() => {
    if (runOnMount) void trigger(false);
  }, [runOnMount, trigger]);

  return (
    <Button
      ref={button}
      variant='ghost'
      size='icon'
      aria-label={label}
      title={label}
      onClick={() => void trigger(true)}
      disabled={busy}
      className='size-[30px] rounded-[6px] text-muted-foreground hover:text-body'
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
      <SyncAction
        icon={CloudDownload}
        label='Pull from cloud'
        run={pullFromCloud}
        done='Pulled posts from the cloud'
        failed='Could not pull posts from the cloud'
        runOnMount
      />
      <SyncAction
        icon={CloudUpload}
        label='Push to cloud'
        run={pushToCloud}
        done='Pushed local posts to the cloud'
        failed='Could not push posts to the cloud'
      />
    </>
  );
}
