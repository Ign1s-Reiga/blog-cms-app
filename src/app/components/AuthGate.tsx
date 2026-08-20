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
      <div className='flex h-screen items-center justify-center bg-zinc-50 dark:bg-[#0a0a0a]'>
        <div className='flex items-center gap-2 text-[13px] font-medium text-zinc-500 dark:text-zinc-400'>
          <span className='size-3.5 rounded-full border-2 border-zinc-300 border-t-zinc-500 dark:border-zinc-700 dark:border-t-zinc-400 animate-spin' />
          Checking session…
        </div>
      </div>
    );
  }

  // No session → render the app behind a blocking login modal.
  if (phase === 'unauthed') {
    return (
      <>
        {children}
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
          className='fixed bottom-4 right-4 z-50 max-w-[420px] rounded-[6px] border border-red-200 dark:border-red-500/[0.3] bg-red-50 dark:bg-red-500/[0.08] px-3 py-2'
        >
          <p className='text-[12px] font-semibold text-red-700 dark:text-red-400'>
            Could not read this account&apos;s posts
          </p>
          <p className='mt-0.5 text-[11px] leading-[1.5] text-red-600 dark:text-red-400/90'>{pullError}</p>
          <button
            type='button'
            onClick={() => setPullError(null)}
            className='mt-1.5 text-[11px] font-semibold text-red-700 dark:text-red-400 underline underline-offset-2'
          >
            Dismiss
          </button>
        </div>
      )}
      {children}
    </>
  );
}
