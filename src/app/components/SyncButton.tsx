"use client";

import { useState } from "react";
import { RefreshCw } from "lucide-react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

/// Push local posts up to Cloudflare D1. No-ops outside the Tauri runtime
/// (e.g. plain `pnpm dev`), where the backend isn't available.
export function SyncButton() {
  const [syncing, setSyncing] = useState(false);

  const handleSync = async () => {
    if (syncing) return;
    const { invoke, isTauri } = await import("@tauri-apps/api/core");
    if (!isTauri()) return;
    setSyncing(true);
    try {
      await invoke<number>("sync_posts");
    } catch (err) {
      console.error("Sync failed:", err);
    } finally {
      setSyncing(false);
    }
  };

  return (
    <Button
      variant="ghost"
      size="icon"
      aria-label="Sync to cloud"
      title="Sync to cloud"
      onClick={handleSync}
      disabled={syncing}
      className="size-[30px] rounded-[6px] text-zinc-400 dark:text-zinc-500 hover:text-zinc-700 dark:hover:text-zinc-300"
    >
      <RefreshCw size={15} strokeWidth={1.8} className={cn(syncing && "animate-spin")} />
    </Button>
  );
}
