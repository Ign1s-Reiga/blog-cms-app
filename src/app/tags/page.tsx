'use client';

import { useCallback, useEffect, useState } from 'react';
import { Check, Loader2, Merge, Pencil, Tags as TagsIcon, X } from 'lucide-react';
import { useRouter } from 'next/navigation';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Alert, AlertDescription } from '@/components/ui/alert';
import { onPostsRefreshed } from '@/lib/sync';

/// Mirrors `TagCount` in `src-tauri/src/commands/local_db.rs`.
type TagCount = { name: string; posts: number };

/// Mirrors `TagRenamed`. `skipped` are posts carrying the tag that were left
/// alone because the body on this machine cannot stand in for the one the post
/// actually has — rewriting them could not be marked as an edit, and an unmarked
/// row is one the next Refresh would quietly overwrite from the cloud.
///
/// `reason` mirrors `SkipReason`: the text was never fetched, or what was
/// fetched has been overtaken. Same remedy, different sentence.
type SkipReason = 'body_not_cached' | 'body_stale';
type TagRenamed = { changed: number; skipped: { id: number; title: string; reason: SkipReason }[] };

type Editing = { from: string; to: string } | null;

export default function TagsPage() {
  const router = useRouter();
  const [tags, setTags] = useState<TagCount[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [editing, setEditing] = useState<Editing>(null);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<(TagRenamed & { from: string; to: string }) | null>(null);

  const load = useCallback(async () => {
    const { invoke, isTauri } = await import('@tauri-apps/api/core');
    if (!isTauri()) {
      setLoading(false);
      return;
    }
    try {
      setTags(await invoke<TagCount[]>('list_tags'));
      setError(null);
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  // A refresh from the cloud rewrites the library's metadata, tags included.
  useEffect(() => onPostsRefreshed(() => void load()), [load]);

  const commit = async () => {
    if (!editing || busy) return;
    const to = editing.to.trim();
    if (to === '' || to === editing.from) {
      setEditing(null);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const { invoke } = await import('@tauri-apps/api/core');
      const done = await invoke<TagRenamed>('rename_tag', { from: editing.from, to });
      setResult({ ...done, from: editing.from, to });
      setEditing(null);
      await load();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  };

  /// Whether the name being typed already belongs to another tag — in which
  /// case this rename is a merge, and the button should say so before it is
  /// pressed rather than afterwards.
  const mergesInto = (candidate: string) => {
    const to = candidate.trim();
    return to !== '' && tags.some((t) => t.name === to && t.name !== editing?.from);
  };

  return (
    <main className='flex-1 overflow-y-auto p-6'>
      <div className='max-w-[720px] space-y-4'>
        <div>
          <h1 className='text-[15px] font-semibold text-zinc-800 dark:text-zinc-200'>Tags</h1>
          <p className='text-[12px] text-zinc-500 dark:text-zinc-600'>
            Every tag in the library, as it is stored. Renaming one onto another merges them.
          </p>
        </div>

        {error && (
          <Alert variant='destructive'>
            <AlertDescription className='text-[12px]'>{error}</AlertDescription>
          </Alert>
        )}

        {result && (
          <Alert className='items-start rounded-[6px] px-3 py-2 border-emerald-200 bg-emerald-50 dark:border-emerald-500/20 dark:bg-emerald-500/[0.08]'>
            <Check size={13} strokeWidth={2} className='mt-[2px] size-3.5 text-emerald-600 dark:text-emerald-400' />
            <AlertDescription className='text-[12px] text-emerald-700 dark:text-emerald-400'>
              <span className='font-mono font-semibold'>{result.from}</span> → {}
              <span className='font-mono font-semibold'>{result.to}</span> on {result.changed} post
              {result.changed === 1 ? '' : 's'}. They are edited locally and go up on the next push.
            </AlertDescription>
          </Alert>
        )}

        {/* A rename that visibly did not finish, rather than one that silently
            comes undone on the next Refresh. */}
        {result && result.skipped.length > 0 && (
          <Alert className='items-start rounded-[6px] px-3 py-2 border-amber-200 bg-amber-50/60 dark:border-amber-900/40 dark:bg-amber-950/20'>
            <AlertDescription className='text-[12px] leading-[1.6] text-amber-700 dark:text-amber-500'>
              {result.skipped.length} post{result.skipped.length === 1 ? '' : 's'} carrying{' '}
              <span className='font-mono font-semibold'>{result.from}</span>{' '}
              {result.skipped.length === 1 ? 'was' : 'were'} left unchanged:{' '}
              {result.skipped.every((p) => p.reason === 'body_stale')
                ? "the copy of their text here is behind the cloud's, so recording the rename against it would put this machine's older text forward as the newer one"
                : result.skipped.every((p) => p.reason === 'body_not_cached')
                  ? 'their text is not on this machine, so the rename could not be recorded as an edit — and an unrecorded edit is one the next refresh would overwrite from the cloud'
                  : 'the text here is either missing or behind the cloud, so the rename could not be recorded against it'}
              . Open {result.skipped.length === 1 ? 'it' : 'them'} once to bring the current text down, then rename
              again.
              <span className='mt-1 block'>
                {result.skipped.map((p, i) => (
                  <span key={p.id}>
                    {i > 0 && ', '}
                    <span className='font-medium'>{p.title}</span>
                  </span>
                ))}
              </span>
            </AlertDescription>
          </Alert>
        )}

        {loading ? (
          <p className='flex items-center gap-2 p-3 text-[12px] text-zinc-500 dark:text-zinc-500'>
            <Loader2 size={13} strokeWidth={2} className='animate-spin' />
            Reading tags…
          </p>
        ) : tags.length === 0 ? (
          <div className='rounded-[8px] border border-dashed border-zinc-200 px-4 py-8 text-center dark:border-white/[0.08]'>
            <TagsIcon size={18} strokeWidth={1.6} className='mx-auto text-zinc-300 dark:text-zinc-700' />
            <p className='mt-2 text-[12px] text-zinc-500 dark:text-zinc-600'>
              No tags yet. Add them to a post from its editor.
            </p>
          </div>
        ) : (
          <ul className='divide-y divide-zinc-100 rounded-[8px] border border-zinc-200 bg-white dark:divide-white/[0.05] dark:border-white/[0.07] dark:bg-[#161616]'>
            {tags.map((tag) => (
              <li key={tag.name} className='px-3 py-2'>
                {editing?.from === tag.name ? (
                  <div className='flex items-center gap-2'>
                    <Input
                      autoFocus
                      value={editing.to}
                      onChange={(e) => setEditing({ ...editing, to: e.target.value })}
                      onKeyDown={(e) => {
                        if (e.key === 'Enter') void commit();
                        if (e.key === 'Escape') setEditing(null);
                      }}
                      spellCheck={false}
                      className='h-[28px] flex-1 font-mono text-[12px]'
                    />
                    <Button
                      size='sm'
                      onClick={() => void commit()}
                      disabled={busy || editing.to.trim() === ''}
                      className='h-[28px] gap-1.5 text-[11px] font-semibold'
                    >
                      {mergesInto(editing.to) && <Merge size={11} strokeWidth={2} />}
                      {busy ? 'Working…' : mergesInto(editing.to) ? 'Merge' : 'Rename'}
                    </Button>
                    <Button
                      variant='ghost'
                      size='icon'
                      aria-label='Cancel'
                      onClick={() => setEditing(null)}
                      disabled={busy}
                      className='size-[26px] rounded-[5px] text-zinc-400 dark:text-zinc-500'
                    >
                      <X size={13} strokeWidth={2} />
                    </Button>
                  </div>
                ) : (
                  <div className='group flex items-center gap-3'>
                    <button
                      type='button'
                      onClick={() => router.push(`/posts?tag=${encodeURIComponent(tag.name)}`)}
                      title={`Show the ${tag.posts} post${tag.posts === 1 ? '' : 's'} tagged ${tag.name}`}
                      className='flex min-w-0 flex-1 items-baseline gap-2.5 rounded-[5px] px-1 py-0.5 text-left transition-colors hover:bg-zinc-50 dark:hover:bg-white/[0.03]'
                    >
                      <span className='truncate font-mono text-[12px] font-semibold text-zinc-700 dark:text-zinc-300'>
                        {tag.name}
                      </span>
                      <span className='shrink-0 text-[11px] tabular-nums text-zinc-400 dark:text-zinc-600'>
                        {tag.posts} post{tag.posts === 1 ? '' : 's'}
                      </span>
                    </button>
                    <Button
                      variant='ghost'
                      size='icon'
                      aria-label={`Rename ${tag.name}`}
                      onClick={() => setEditing({ from: tag.name, to: tag.name })}
                      className='size-[26px] shrink-0 rounded-[5px] text-zinc-400 opacity-0 transition-opacity group-hover:opacity-100 focus-visible:opacity-100 dark:text-zinc-500'
                    >
                      <Pencil size={12} strokeWidth={2} />
                    </Button>
                  </div>
                )}
              </li>
            ))}
          </ul>
        )}

        {tags.length > 0 && (
          <p className='text-[11px] leading-[1.6] text-zinc-500 dark:text-zinc-600'>
            Tags are listed exactly as they are stored, so <span className='font-mono'>Rust</span> and{' '}
            <span className='font-mono'>rust</span> appear separately — which is how you find the ones worth merging.
            Search treats them as the same, which is what hides the difference elsewhere.
          </p>
        )}
      </div>
    </main>
  );
}
