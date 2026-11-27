// Local-first data sync helpers.
//
// The UI reads posts from the local cache (`list_posts`). Cloud sync is explicit
// and directional:
//   - `pullFromCloud` mirrors D1 → local (cloud wins; also runs on launch/login).
//   - `pushToCloud`   pushes local → D1 (upsert by slug).
// Both broadcast `POSTS_REFRESHED` afterwards so the list, dashboard, and sidebar
// re-read local data.

export const POSTS_REFRESHED = 'posts:refreshed';

function broadcastPostsRefreshed(): void {
  if (typeof window !== 'undefined') {
    window.dispatchEvent(new Event(POSTS_REFRESHED));
  }
}

/// Pull the connected account's posts from D1 into the local cache (mirror,
/// cloud wins), then notify listeners to re-read. No-ops outside the Tauri
/// runtime (e.g. plain `pnpm dev`).
///
/// Resolves to whether a sync ran, so a caller that reports success does not
/// claim one for the no-op.
export async function pullFromCloud(): Promise<boolean> {
  const { invoke, isTauri } = await import('@tauri-apps/api/core');
  if (!isTauri()) return false;
  await invoke<number>('sync_posts_from_cloud');
  broadcastPostsRefreshed();
  return true;
}

/// Push local posts up to Cloudflare D1 (upsert by slug), then notify listeners
/// to re-read (local sync status may change). No-ops outside the Tauri runtime,
/// and resolves to whether a sync ran, as `pullFromCloud` does.
export async function pushToCloud(): Promise<boolean> {
  const { invoke, isTauri } = await import('@tauri-apps/api/core');
  if (!isTauri()) return false;
  await invoke<number>('sync_posts');
  broadcastPostsRefreshed();
  return true;
}

/// What a sync click says when there is nothing to sync with: the app is
/// running in a plain browser, which has no backend.
export const SYNC_UNAVAILABLE = 'Cloud sync runs in the desktop app only';

/// Subscribe to post-refresh notifications. Returns an unsubscribe function
/// suitable for returning directly from a `useEffect`.
export function onPostsRefreshed(handler: () => void): () => void {
  if (typeof window === 'undefined') return () => {};
  window.addEventListener(POSTS_REFRESHED, handler);
  return () => window.removeEventListener(POSTS_REFRESHED, handler);
}
