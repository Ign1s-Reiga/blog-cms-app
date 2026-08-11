// Self-update helpers, backed by GitHub Releases.
//
// The Rust side (`src-tauri/src/update.rs`) does the network work and the
// signature check; this module is the thin invoke wrapper plus a small cache so
// the app checks GitHub once per launch and every interested component reads the
// same answer:
//   - `checkForUpdate` fetches (or replays) the latest status.
//   - `UPDATE_STATUS_CHANGED` broadcasts a new status to listeners.
// All calls no-op outside the Tauri runtime (e.g. plain `pnpm dev`).

export const UPDATE_STATUS_CHANGED = "update:status-changed";

export interface UpdateStatus {
  available: boolean;
  current_version: string;
  version: string | null;
  notes: string | null;
  date: string | null;
}

export interface DownloadProgress {
  downloaded: number;
  /// Absent when the release server sends no content length.
  total: number | null;
}

/// The most recent successful check, shared across components.
let lastStatus: UpdateStatus | null = null;
/// In-flight check, so concurrent callers await one request instead of racing.
let inFlight: Promise<UpdateStatus | null> | null = null;

export function getCachedStatus(): UpdateStatus | null {
  return lastStatus;
}

/// Check GitHub Releases for a newer version.
///
/// Repeat calls replay the cached result; pass `force` to re-query (the manual
/// "Check for updates" button). Returns `null` outside the Tauri runtime.
export async function checkForUpdate(force = false): Promise<UpdateStatus | null> {
  const { invoke, isTauri } = await import("@tauri-apps/api/core");
  if (!isTauri()) return null;

  if (!force && lastStatus) return lastStatus;
  if (inFlight) return inFlight;

  inFlight = invoke<UpdateStatus>("check_for_update")
    .then((status) => {
      lastStatus = status;
      broadcastStatus(status);
      return status;
    })
    .finally(() => {
      inFlight = null;
    });

  return inFlight;
}

/// Download and install the update found by the last check.
///
/// `onProgress` fires as bytes arrive. On Windows the installer takes over once
/// the download finishes and closes the app, so this generally does not return
/// on that platform — treat reaching the end as "ready to restart" elsewhere.
export async function installUpdate(
  onProgress?: (progress: DownloadProgress) => void,
): Promise<void> {
  const { invoke, isTauri } = await import("@tauri-apps/api/core");
  if (!isTauri()) return;

  const { listen } = await import("@tauri-apps/api/event");
  const unlisten = onProgress
    ? await listen<DownloadProgress>("update://download-progress", (e) => onProgress(e.payload))
    : undefined;

  try {
    await invoke("install_update");
  } finally {
    unlisten?.();
  }
}

/// Relaunch into the newly installed version.
export async function restartApp(): Promise<void> {
  const { invoke, isTauri } = await import("@tauri-apps/api/core");
  if (!isTauri()) return;
  await invoke("restart_app");
}

/// Subscribe to update-status changes. Returns an unsubscribe function suitable
/// for returning directly from a `useEffect`.
export function onUpdateStatusChanged(handler: (status: UpdateStatus) => void): () => void {
  if (typeof window === "undefined") return () => {};
  const listener = (e: Event) => handler((e as CustomEvent<UpdateStatus>).detail);
  window.addEventListener(UPDATE_STATUS_CHANGED, listener);
  return () => window.removeEventListener(UPDATE_STATUS_CHANGED, listener);
}

function broadcastStatus(status: UpdateStatus): void {
  if (typeof window !== "undefined") {
    window.dispatchEvent(new CustomEvent(UPDATE_STATUS_CHANGED, { detail: status }));
  }
}
