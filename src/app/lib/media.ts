/// What kind of thing a media object is, and the Markdown that embeds it.
///
/// The library holds images and video, and the two are not interchangeable at
/// the point of insertion: a video written as `![alt](…)` is an image tag whose
/// source is not an image, which renders as a broken image and nothing else.
/// Both the picker and the editor decide from here, so they cannot disagree.

/// Extensions stored as video. Mirrors the video half of `MEDIA_EXTS` in
/// `src-tauri/src/commands/r2.rs`.
const VIDEO_EXT = /\.(?:mp4|webm|mov)$/i;

export type MediaKind = 'image' | 'video';

export function mediaKind(name: string): MediaKind {
  return VIDEO_EXT.test(name) ? 'video' : 'image';
}

export function isVideo(name: string): boolean {
  return mediaKind(name) === 'video';
}

/// The Markdown to insert for a staged object at `rel` (an `assets/…` path).
///
/// Video goes in as raw HTML, because Markdown has no syntax for it. That is
/// safe to do here: `extract_asset_refs` finds an `assets/…` reference wherever
/// it appears — it stops at a quote as readily as at a `)` — so the publish path
/// uploads and rewrites the source of a `<video>` exactly as it does an image's.
///
/// `preload="metadata"` so a post with a video on it does not cost every reader
/// the whole file before they have asked for it.
///
/// The source is double-quoted deliberately. `resolveAssetSrcs` in the editor
/// rewrites `src="assets/…"` for the preview and matches double quotes only, so
/// a single-quoted source would publish correctly and show as a dead video while
/// it was being written.
export function mediaMarkup(rel: string, name: string): string {
  const alt = name.replace(/\.[^.]+$/, '');
  return mediaKind(name) === 'video' ? `<video controls preload="metadata" src="${rel}"></video>` : `![${alt}](${rel})`;
}
