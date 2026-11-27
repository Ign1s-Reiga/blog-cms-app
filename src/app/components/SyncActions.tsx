'use client';

import { useCallback, useEffect, useRef, useState } from 'react';
import { CloudDownload, CloudUpload, type LucideIcon } from 'lucide-react';
import { toast } from 'sonner';
import { Button } from '@/components/ui/button';
import { cn } from '@/lib/utils';
import { SYNC_UNAVAILABLE, pullFromCloud, pushToCloud } from '@/lib/sync';

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
  /// Resolves to whether the sync ran; `false` outside the desktop app.
  run: () => Promise<boolean>;
  /// What a toast says when a click succeeded, and when it failed.
  done: string;
  failed: string;
  runOnMount?: boolean;
}) {
  const [busy, setBusy] = useState(false);
  const guard = useRef(false);
  const button = useRef<HTMLButtonElement>(null);
  /// Whether this control is still on screen. A launch-time run that outlives
  /// it belongs to a shell that is gone, and has no one to report to.
  const mounted = useRef(false);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  const trigger = useCallback(
    async (clicked: boolean) => {
      if (guard.current) return;
      guard.current = true;
      // `AuthGate` renders the app inert while it asks for credentials. Read it
      // as the run starts, not when it fails: signing in meanwhile remounts the
      // shell, and by then this button is detached and would no longer say it
      // had been behind the login — so a pull that had no credentials would
      // report its failure into the session that does.
      const behindLogin = button.current?.closest('[inert]') != null;
      setBusy(true);
      try {
        const ran = await run();
        if (clicked) {
          if (ran) toast.success(done);
          else toast(SYNC_UNAVAILABLE);
        }
      } catch (err) {
        console.error(`${label} failed:`, err);
        if (clicked) {
          toast.error(failed, { description: String(err) });
        } else if (!behindLogin && mounted.current) {
          // Nobody asked for this run, so nobody is watching for its result.
          // It stays until dismissed: it is how a bad token from signing in
          // shows, and once a fading toast is gone, the empty library behind
          // it reads as an account with no posts.
          toast.error(failed, { description: String(err), duration: Infinity, closeButton: true });
        }
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
