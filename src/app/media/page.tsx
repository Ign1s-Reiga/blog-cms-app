"use client";

import { useCallback, useEffect, useState } from "react";
import { Trash2, Upload } from "lucide-react";
import { Button } from "@/components/ui/button";

type MediaItem = {
  key: string;
  name: string;
  size: number;
  src: string;
  isVideo: boolean;
};

const VIDEO_EXT = /\.(?:mp4|webm|mov)$/i;

function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

export default function MediaPage() {
  const [items, setItems]     = useState<MediaItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy]       = useState(false);
  const [error, setError]     = useState<string | null>(null);

  // Load media from R2 (cached locally by the backend). No-ops in a plain
  // browser (`pnpm dev`), where the Tauri API isn't available.
  const loadMedia = useCallback(async () => {
    const { invoke, isTauri, convertFileSrc } = await import("@tauri-apps/api/core");
    if (!isTauri()) {
      setLoading(false);
      return;
    }
    const { appDataDir, join } = await import("@tauri-apps/api/path");
    try {
      const rows = await invoke<{ key: string; name: string; size: number }[]>("list_media");
      const base = await appDataDir();
      const resolved = await Promise.all(
        rows.map(async (r) => ({
          ...r,
          // The key doubles as the local-relative cache path.
          src: convertFileSrc(await join(base, r.key)),
          isVideo: VIDEO_EXT.test(r.name),
        })),
      );
      setItems(resolved);
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadMedia();
  }, [loadMedia]);

  const handleUpload = async () => {
    const { invoke, isTauri } = await import("@tauri-apps/api/core");
    if (!isTauri()) return;
    setBusy(true);
    try {
      await invoke("upload_media");
      await loadMedia();
    } catch (e) {
      const msg = String(e);
      if (msg !== "cancelled") setError(msg);
    } finally {
      setBusy(false);
    }
  };

  const handleDelete = async (key: string) => {
    const { invoke, isTauri } = await import("@tauri-apps/api/core");
    if (!isTauri()) return;
    try {
      await invoke("delete_media", { key });
      setItems((prev) => prev.filter((i) => i.key !== key));
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <main className="flex-1 overflow-y-auto p-6">
      <div className="space-y-4 w-full">
        {/* Toolbar */}
        <div className="flex items-center justify-between gap-4">
          <div className="flex items-baseline gap-2">
            <h1 className="text-[15px] font-semibold text-zinc-800 dark:text-zinc-200">Media Library</h1>
            <span className="text-[12px] text-zinc-400 dark:text-zinc-600">
              {items.length} {items.length === 1 ? "file" : "files"}
            </span>
          </div>
          <Button
            size="sm"
            onClick={handleUpload}
            disabled={busy}
            className="h-[30px] px-3 gap-[6px] rounded-[6px] text-[13px] font-semibold"
          >
            <Upload size={13} strokeWidth={2} />
            {busy ? "Uploading…" : "Upload"}
          </Button>
        </div>

        {error && (
          <div className="rounded-[6px] px-3 py-2 border border-red-200 bg-red-50 text-red-700 dark:border-red-500/20 dark:bg-red-500/[0.08] dark:text-red-400 text-[12px] font-medium">
            {error}
          </div>
        )}

        {loading ? (
          <p className="py-16 text-center text-[13px] text-zinc-400 dark:text-zinc-600">Loading media…</p>
        ) : items.length === 0 ? (
          <p className="py-16 text-center text-[13px] text-zinc-400 dark:text-zinc-600">
            No media yet. Upload an image or video to get started.
          </p>
        ) : (
          <div className="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-4 xl:grid-cols-5 gap-3">
            {items.map((item) => (
              <div
                key={item.key}
                className="group relative rounded-[8px] border border-zinc-200 dark:border-white/[0.07] overflow-hidden bg-white dark:bg-[#161616]"
              >
                <div className="aspect-square bg-zinc-50 dark:bg-white/[0.02] flex items-center justify-center overflow-hidden">
                  {item.isVideo ? (
                    <video src={item.src} muted preload="metadata" className="w-full h-full object-cover" />
                  ) : (
                    // eslint-disable-next-line @next/next/no-img-element
                    <img src={item.src} alt={item.name} loading="lazy" className="w-full h-full object-cover" />
                  )}
                </div>
                <div className="flex items-center justify-between gap-2 px-2.5 py-2 border-t border-zinc-100 dark:border-white/[0.05]">
                  <div className="min-w-0">
                    <p className="text-[11px] font-mono text-zinc-600 dark:text-zinc-400 truncate">{item.name}</p>
                    <p className="text-[10px] text-zinc-400 dark:text-zinc-600">{formatSize(item.size)}</p>
                  </div>
                  <button
                    type="button"
                    aria-label={`Delete ${item.name}`}
                    onClick={() => handleDelete(item.key)}
                    className="shrink-0 p-1 rounded-[4px] text-zinc-400 hover:text-red-600 dark:hover:text-red-400 hover:bg-red-50 dark:hover:bg-red-500/[0.1] transition-colors opacity-0 group-hover:opacity-100"
                  >
                    <Trash2 size={13} strokeWidth={2} />
                  </button>
                </div>
              </div>
            ))}
          </div>
        )}
      </div>
    </main>
  );
}
