"use client";

import { useCallback, useEffect, useState } from "react";
import { LogOut } from "lucide-react";
import { Button } from "@/components/ui/button";
import { UpdateCard } from "@/components/UpdateCard";

type Creds = { account_id: string; r2_bucket: string; d1_database_id: string };

export default function SettingsPage() {
  const [creds, setCreds]     = useState<Creds | null>(null);
  const [loading, setLoading] = useState(true);

  const load = useCallback(async () => {
    const { invoke, isTauri } = await import("@tauri-apps/api/core");
    if (!isTauri()) {
      setLoading(false);
      return;
    }
    try {
      setCreds(await invoke<Creds | null>("get_credentials"));
    } catch {
      // ignore
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const signOut = async () => {
    const { invoke, isTauri } = await import("@tauri-apps/api/core");
    if (!isTauri()) return;
    try {
      await invoke("clear_credentials");
      window.location.reload(); // re-run the AuthGate → back to the login screen
    } catch {
      // ignore
    }
  };

  return (
    <main className="flex-1 overflow-y-auto p-6">
      <div className="max-w-[560px] space-y-6">
        <div>
          <h1 className="text-[15px] font-semibold text-zinc-800 dark:text-zinc-200">Settings</h1>
          <p className="text-[12px] text-zinc-500 dark:text-zinc-600">
            Cloudflare connection and preferences.
          </p>
        </div>

        <section className="rounded-[8px] border border-zinc-200 dark:border-white/[0.07] bg-white dark:bg-[#161616]">
          <div className="px-4 py-3 border-b border-zinc-100 dark:border-white/[0.05]">
            <h2 className="text-[13px] font-semibold text-zinc-800 dark:text-zinc-200">Cloudflare</h2>
          </div>
          <div className="px-4 py-3 space-y-2 text-[12px]">
            {loading ? (
              <p className="text-zinc-400 dark:text-zinc-600">Loading…</p>
            ) : creds ? (
              <>
                <Row label="Account ID" value={creds.account_id} />
                <Row label="R2 Bucket" value={creds.r2_bucket} />
                <Row label="D1 Database" value={creds.d1_database_id} />
                <div className="pt-2">
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={signOut}
                    className="h-[30px] gap-1.5 text-[12px] font-semibold text-red-600 dark:text-red-400"
                  >
                    <LogOut size={13} strokeWidth={2} />
                    Sign out
                  </Button>
                </div>
              </>
            ) : (
              <p className="text-zinc-400 dark:text-zinc-600">Not connected.</p>
            )}
          </div>
        </section>

        <UpdateCard />
      </div>
    </main>
  );
}

function Row({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex items-center justify-between gap-4">
      <span className="text-zinc-500 dark:text-zinc-500">{label}</span>
      <span className="max-w-[300px] truncate font-mono text-zinc-700 dark:text-zinc-300">{value}</span>
    </div>
  );
}
