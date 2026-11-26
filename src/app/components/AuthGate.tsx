'use client';

import { useCallback, useEffect, useState } from 'react';
import { LoginScreen } from '@/components/LoginScreen';
import { pullFromCloud } from '@/lib/sync';

type Phase = 'checking' | 'authed' | 'unauthed';

// Gates the app behind a Cloudflare session. On mount it verifies the stored
// credentials (session_status); until that resolves it shows "Checking session…".
// In a plain browser (`pnpm dev`) there's no backend, so it shows the app.
export function AuthGate({ children }: { children: React.ReactNode }) {
  const [phase, setPhase] = useState<Phase>('checking');
  /// A first pull that failed after signing in — almost always the credentials,
  /// since nothing has checked them before this point.
  const [pullError, setPullError] = useState<string | null>(null);

  const check = useCallback(async () => {
    const { invoke, isTauri } = await import('@tauri-apps/api/core');
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
        <LoginScreen
          onAuthed={() => {
            setPhase('authed');
            // Pull the freshly connected account's posts into the local cache.
            //
            // `LoginScreen` accepts a token without checking it, on the
            // grounds that a bad one surfaces when the app talks to R2/D1 —
            // this pull is that moment, so its error has to be caught or the
            // empty local cache reads as an account with no posts.
            //
            // Shown beside the app rather than in front of it: everything local
            // still works, and what failed was reaching Cloudflare.
            void pullFromCloud().catch((err: unknown) => {
              setPullError(String(err));
            });
          }}
        />
      </>
    );
  }

  return (
    <>
      {pullError !== null && (
        <div
          role='alert'
          className='fixed bottom-4 right-4 z-50 max-w-[420px] rounded-[6px] border border-destructive/30 bg-destructive/10 px-3 py-2'
        >
          <p className='text-meta font-semibold text-destructive'>Could not read this account&apos;s posts</p>
          <p className='mt-0.5 text-label leading-[1.5] text-destructive'>{pullError}</p>
          <button
            type='button'
            onClick={() => setPullError(null)}
            className='mt-1.5 text-label font-semibold text-destructive underline underline-offset-2'
          >
            Dismiss
          </button>
        </div>
      )}
      {children}
    </>
  );
}
