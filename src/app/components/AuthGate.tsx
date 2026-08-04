"use client";

import { useCallback, useEffect, useState } from "react";
import { LoginScreen } from "./LoginScreen";

type Phase = "checking" | "authed" | "unauthed";

// Gates the app behind a Cloudflare session. On mount it verifies the stored
// credentials (session_status); until that resolves it shows "Checking session…".
// In a plain browser (`pnpm dev`) there's no backend, so it shows the app.
export function AuthGate({ children }: { children: React.ReactNode }) {
  const [phase, setPhase] = useState<Phase>("checking");

  const check = useCallback(async () => {
    const { invoke, isTauri } = await import("@tauri-apps/api/core");
    if (!isTauri()) {
      setPhase("authed");
      return;
    }
    setPhase("checking");
    try {
      const status = await invoke<{ authenticated: boolean }>("session_status");
      setPhase(status.authenticated ? "authed" : "unauthed");
    } catch {
      setPhase("unauthed");
    }
  }, []);

  useEffect(() => {
    void check();
  }, [check]);

  if (phase === "checking") {
    return (
      <div className="flex h-screen items-center justify-center bg-zinc-50 dark:bg-[#0a0a0a]">
        <div className="flex items-center gap-2 text-[13px] font-medium text-zinc-500 dark:text-zinc-400">
          <span className="size-3.5 rounded-full border-2 border-zinc-300 border-t-zinc-500 dark:border-zinc-700 dark:border-t-zinc-400 animate-spin" />
          Checking session…
        </div>
      </div>
    );
  }

  if (phase === "unauthed") {
    return <LoginScreen onAuthed={() => setPhase("authed")} />;
  }

  return <>{children}</>;
}
