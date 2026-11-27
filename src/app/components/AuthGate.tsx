'use client';

import { useCallback, useEffect, useState } from 'react';
import { LoginScreen } from '@/components/LoginScreen';
import { invoke, isTauri } from '@tauri-apps/api/core';

type Phase = 'checking' | 'authed' | 'unauthed';

// Gates the app behind a Cloudflare session. On mount it verifies the stored
// credentials (session_status); until that resolves it shows "Checking session…".
// In a plain browser (`pnpm dev`) there's no backend, so it shows the app.
export function AuthGate({ children }: { children: React.ReactNode }) {
  const [phase, setPhase] = useState<Phase>('checking');

  const check = useCallback(async () => {
    if (!isTauri()) {
      setPhase('authed');
      return;
    }
    setPhase('checking');
    try {
      // Never sit on "Checking session" — if the backend is slow/unavailable,
      // fall back to the login modal after a few seconds.
      const status = await Promise.race([
        invoke<{ authenticated: boolean }>('session_status'),
        new Promise<{ authenticated: boolean }>((_, reject) =>
          setTimeout(() => reject(new Error('session check timed out')), 4000),
        ),
      ]);
      setPhase(status.authenticated ? 'authed' : 'unauthed');
    } catch {
      setPhase('unauthed');
    }
  }, []);

  useEffect(() => {
    void check();
  }, [check]);

  if (phase === 'checking') {
    return (
      <div className='flex h-screen items-center justify-center bg-muted/50'>
        <div className='flex items-center gap-2 text-ui font-medium text-muted-foreground'>
          <span className='size-3.5 rounded-full border-2 border-input border-t-muted-foreground animate-spin' />
          Checking session…
        </div>
      </div>
    );
  }

  // No session → render the app behind a blocking login modal.
  if (phase === 'unauthed') {
    return (
      <>
        {/* Seen but not reachable: `inert` keeps Tab and the pointer out of the
            app behind the login, and `aria-hidden` keeps a screen reader from
            reading it as though it were usable. `contents` leaves the layout
            exactly as it was. Signing in still remounts the app, as it did
            before the wrapper, so every screen reloads against the new
            session. */}
        <div className='contents' inert aria-hidden='true'>
          {children}
        </div>
        {/* Signing in remounts the app, and the header's launch-time pull
            runs again as it mounts — now with the new account's credentials.
            That pull is the first thing to talk to R2/D1, so it is where a bad
            token shows up (`LoginScreen` accepts one without checking it), and
            it reports a failure as a toast beside the app — one that stays until
            dismissed, since nobody clicked for it: everything local
            still works, and what failed was reaching Cloudflare. A second pull
            here would only race it and report the same error twice. */}
        <LoginScreen onAuthed={() => setPhase('authed')} />
      </>
    );
  }

  return <>{children}</>;
}
