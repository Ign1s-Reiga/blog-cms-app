'use client';

import { useCallback, useEffect, useId, useRef, useState } from 'react';
import { Layers, Loader2, Pencil, Plus, Trash2, X } from 'lucide-react';
import { useRouter } from 'next/navigation';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Alert, AlertDescription } from '@/components/ui/alert';
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog';
import { onPostsRefreshed } from '@/lib/sync';
import { cn } from '@/lib/utils';

/// Mirrors `series::Model` in `src-tauri/src/entities/series.rs`.
type Series = {
  id: number;
  slug: string;
  title: string;
  description: string | null;
  created_at: number;
};

/// Only the fields this screen needs off a post.
type PostRow = {
  id: number;
  title: string;
  series_id: number | null;
  series_order: number | null;
};

/// A series with the posts filed under it, in reading order.
type SeriesWithPosts = Series & { posts: PostRow[] };

/// `null` means the form is closed; an id means it is editing that series, and
/// `'new'` that it is creating one.
type Editing = null | 'new' | number;

/// Posts with an order come first, in it; the rest follow by title, so a series
/// nobody has ordered yet still reads in a stable sequence.
function inReadingOrder(posts: PostRow[]): PostRow[] {
  return [...posts].sort((a, b) => {
    if (a.series_order !== null && b.series_order !== null) return a.series_order - b.series_order;
    if (a.series_order !== null) return -1;
    if (b.series_order !== null) return 1;
    return a.title.localeCompare(b.title);
  });
}

export default function SeriesPage() {
  const router = useRouter();
  const [series, setSeries] = useState<SeriesWithPosts[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const [editing, setEditing] = useState<Editing>(null);
  const [title, setTitle] = useState('');
  const [description, setDescription] = useState('');
  const [busy, setBusy] = useState(false);

  /// The series whose deletion is waiting on an answer. Held whole rather than
  /// by id, because the confirmation has to say how many posts it will unfile.
  const [confirming, setConfirming] = useState<SeriesWithPosts | null>(null);
  /// The delete button that opened the confirmation. Radix returns focus to a
  /// dialog's trigger, and this one has none — it opens from a button per row.
  const returnFocus = useRef<HTMLElement | null>(null);

  const load = useCallback(async () => {
    const { invoke, isTauri } = await import('@tauri-apps/api/core');
    if (!isTauri()) {
      setLoading(false);
      return;
    }
    try {
      // Posts carry the membership, so both lists are needed to say what a
      // series holds. Both are local reads.
      const [rows, posts] = await Promise.all([invoke<Series[]>('list_series'), invoke<PostRow[]>('list_posts')]);
      setSeries(
        rows.map((s) => ({
          ...s,
          posts: inReadingOrder(posts.filter((p) => p.series_id === s.id)),
        })),
      );
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

  // A refresh from the cloud brings series down with the posts, so the list
  // here is stale the moment one lands.
  useEffect(() => onPostsRefreshed(() => void load()), [load]);

  const openNew = () => {
    setEditing('new');
    setTitle('');
    setDescription('');
  };

  const openEdit = (s: Series) => {
    setEditing(s.id);
    setTitle(s.title);
    setDescription(s.description ?? '');
  };

  const closeForm = () => {
    setEditing(null);
    setBusy(false);
  };

  const submit = async () => {
    if (busy || !title.trim()) return;
    setBusy(true);
    setError(null);
    try {
      const { invoke } = await import('@tauri-apps/api/core');
      if (editing === 'new') {
        // `slug` and `created_at` are settled on the Rust side — the slug is
        // the name D1 knows this series by, so it is not the form's to invent.
        await invoke('create_series', {
          series: { id: 0, slug: '', title: title.trim(), description: description.trim() || null, created_at: 0 },
        });
      } else {
        const current = series.find((s) => s.id === editing);
        if (!current) return;
        await invoke('update_series', {
          series: { ...current, title: title.trim(), description: description.trim() || null },
        });
      }
      closeForm();
      await load();
    } catch (err) {
      setError(String(err));
      setBusy(false);
    }
  };

  const remove = async (s: SeriesWithPosts) => {
    setBusy(true);
    setError(null);
    try {
      const { invoke } = await import('@tauri-apps/api/core');
      await invoke<number>('delete_series', { id: s.id });
      setConfirming(null);
      await load();
    } catch (err) {
      // Closed on failure too: the error banner sits behind the dialog, where
      // the modal hides it from sight and from screen readers alike.
      setConfirming(null);
      setError(String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <main className='flex-1 overflow-y-auto p-6'>
      <div className='max-w-[900px] space-y-4'>
        <div className='flex items-start justify-between gap-4'>
          <div>
            <h1 className='text-title font-semibold tracking-tight text-balance text-foreground'>Series</h1>
            <p className='text-meta text-muted-foreground'>
              Groups of related posts. A post is filed into one from its editor.
            </p>
          </div>
          <Button
            size='sm'
            onClick={openNew}
            disabled={editing === 'new'}
            className='h-[30px] gap-1.5 text-meta font-semibold'
          >
            <Plus size={13} strokeWidth={2} />
            New series
          </Button>
        </div>

        {error && (
          <Alert variant='destructive'>
            <AlertDescription className='text-meta'>{error}</AlertDescription>
          </Alert>
        )}

        {editing === 'new' && (
          <SeriesForm
            heading='New series'
            title={title}
            description={description}
            busy={busy}
            onTitle={setTitle}
            onDescription={setDescription}
            onCancel={closeForm}
            onSubmit={() => void submit()}
          />
        )}

        {loading ? (
          <p className='flex items-center gap-2 p-3 text-meta text-muted-foreground'>
            <Loader2 size={13} strokeWidth={2} className='animate-spin' />
            Loading series…
          </p>
        ) : series.length === 0 && editing !== 'new' ? (
          <div className='rounded-[8px] border border-dashed border-border px-4 py-8 text-center'>
            <Layers size={18} strokeWidth={1.6} className='mx-auto text-muted-foreground' />
            <p className='mt-2 text-meta text-muted-foreground'>
              No series yet. Make one, then file posts into it from the editor.
            </p>
          </div>
        ) : (
          <ul className='space-y-3'>
            {series.map((s) => (
              <li key={s.id} className='rounded-[8px] border border-border bg-card'>
                {editing === s.id ? (
                  <SeriesForm
                    heading='Edit series'
                    title={title}
                    description={description}
                    busy={busy}
                    slug={s.slug}
                    onTitle={setTitle}
                    onDescription={setDescription}
                    onCancel={closeForm}
                    onSubmit={() => void submit()}
                  />
                ) : (
                  <>
                    <div className='flex items-start justify-between gap-3 px-4 py-3'>
                      <div className='min-w-0'>
                        <h2 className='truncate text-heading font-semibold text-foreground'>{s.title}</h2>
                        <p className='mt-0.5 font-mono text-label text-muted-foreground'>{s.slug}</p>
                        {s.description && <p className='mt-1.5 text-meta leading-[1.5] text-body'>{s.description}</p>}
                      </div>
                      <div className='flex shrink-0 items-center gap-1'>
                        <Button
                          variant='ghost'
                          size='icon'
                          aria-label={`Edit ${s.title}`}
                          onClick={() => openEdit(s)}
                          className='size-[26px] rounded-[5px] text-muted-foreground'
                        >
                          <Pencil size={13} strokeWidth={2} />
                        </Button>
                        <Button
                          variant='ghost'
                          size='icon'
                          aria-label={`Delete ${s.title}`}
                          onClick={(e) => {
                            returnFocus.current = e.currentTarget;
                            setConfirming(s);
                          }}
                          className='size-[26px] rounded-[5px] text-muted-foreground hover:text-destructive'
                        >
                          <Trash2 size={13} strokeWidth={2} />
                        </Button>
                      </div>
                    </div>

                    <div className='border-t border-border/60 px-2 py-2'>
                      {s.posts.length === 0 ? (
                        <p className='px-2 py-1 text-label text-muted-foreground'>Nothing filed here yet.</p>
                      ) : (
                        <ol className='space-y-px'>
                          {s.posts.map((p, i) => (
                            <li key={p.id}>
                              <button
                                type='button'
                                onClick={() => router.push(`/posts/edit?id=${p.id}`)}
                                className='flex w-full items-baseline gap-2.5 rounded-[5px] px-2 py-1.5 text-left transition-colors hover:bg-muted/50 active:scale-[0.99]'
                              >
                                <span className='w-[18px] shrink-0 text-right font-mono text-label tabular-nums text-muted-foreground'>
                                  {p.series_order ?? i + 1}
                                </span>
                                <span className='truncate text-meta text-body'>{p.title}</span>
                                {p.series_order === null && (
                                  <span className='shrink-0 text-label text-muted-foreground'>no order set</span>
                                )}
                              </button>
                            </li>
                          ))}
                        </ol>
                      )}
                    </div>
                  </>
                )}
              </li>
            ))}
          </ul>
        )}
      </div>

      {/* Not dismissable while the delete is in flight, so Cancel or Escape
          cannot look like they stopped a delete that is already under way. */}
      <AlertDialog
        open={confirming !== null}
        onOpenChange={(open) => {
          if (!open && !busy) setConfirming(null);
        }}
      >
        {confirming && (
          <AlertDialogContent
            onCloseAutoFocus={(e) => {
              e.preventDefault();
              if (returnFocus.current?.isConnected) returnFocus.current.focus();
              returnFocus.current = null;
            }}
            className='gap-0 rounded-[8px] border border-border bg-card p-4 ring-0 data-[size=default]:max-w-[480px] data-[size=default]:sm:max-w-[480px]'
          >
            <AlertDialogTitle className='text-ui font-semibold text-foreground'>
              Delete “{confirming.title}”?
            </AlertDialogTitle>
            <AlertDialogDescription className='mt-2 text-meta leading-[1.6] text-pretty text-body'>
              {confirming.posts.length === 0
                ? 'No posts are filed here, so nothing else changes.'
                : `${confirming.posts.length} post${confirming.posts.length === 1 ? '' : 's'} will be taken out of this series. ${confirming.posts.length === 1 ? 'It is' : 'They are'} not deleted — only unfiled.`}
            </AlertDialogDescription>
            <p className='mt-2 text-label leading-[1.6] text-muted-foreground'>
              The next push takes the posts up unfiled. The copy of this series in the cloud is left alone.
            </p>
            <div className='mt-4 flex items-center justify-end gap-2'>
              <AlertDialogCancel variant='ghost' size='sm' disabled={busy} className='h-[30px] text-meta'>
                Cancel
              </AlertDialogCancel>
              <AlertDialogAction
                variant='destructive'
                size='sm'
                disabled={busy}
                onClick={(e) => {
                  // Held open until the delete settles; `remove` closes it
                  // either way.
                  e.preventDefault();
                  void remove(confirming);
                }}
                className='h-[30px] text-meta font-semibold'
              >
                {busy ? 'Deleting…' : 'Delete series'}
              </AlertDialogAction>
            </div>
          </AlertDialogContent>
        )}
      </AlertDialog>
    </main>
  );
}

/// The create and edit forms are the same shape; only the heading and whether a
/// slug is already settled differ.
function SeriesForm({
  heading,
  title,
  description,
  busy,
  slug,
  onTitle,
  onDescription,
  onCancel,
  onSubmit,
}: {
  heading: string;
  title: string;
  description: string;
  busy: boolean;
  slug?: string;
  onTitle: (v: string) => void;
  onDescription: (v: string) => void;
  onCancel: () => void;
  onSubmit: () => void;
}) {
  const id = useId();
  const titleId = `${id}-title`;
  const descriptionId = `${id}-description`;
  const slugHintId = `${id}-slug-hint`;

  return (
    <div className={cn('rounded-[8px] border border-border bg-card', 'px-4 py-3 space-y-3')}>
      <div className='flex items-center justify-between'>
        <h2 className='text-heading font-semibold text-foreground'>{heading}</h2>
        <Button
          variant='ghost'
          size='icon'
          aria-label='Close'
          onClick={onCancel}
          className='size-[26px] rounded-[5px] text-muted-foreground'
        >
          <X size={13} strokeWidth={2} />
        </Button>
      </div>

      <div className='space-y-1.5'>
        <Label htmlFor={titleId} className='block text-meta leading-normal font-medium text-body'>
          Title
        </Label>
        <Input
          id={titleId}
          aria-describedby={slug !== undefined ? slugHintId : undefined}
          value={title}
          onChange={(e) => onTitle(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') onSubmit();
          }}
          placeholder='Series title'
          className='h-[32px] text-meta'
        />
      </div>

      <div className='space-y-1.5'>
        <Label htmlFor={descriptionId} className='block text-meta leading-normal font-medium text-body'>
          Description
        </Label>
        <Input
          id={descriptionId}
          value={description}
          onChange={(e) => onDescription(e.target.value)}
          placeholder='Optional'
          className='h-[32px] text-meta'
        />
      </div>

      {/* A rename keeps the slug: it is the name D1 knows the series by, and
          moving it would make the renamed series a different one over there. */}
      {slug !== undefined && (
        <p id={slugHintId} className='text-label text-muted-foreground'>
          The slug stays <span className='font-mono text-body'>{slug}</span> — it is how the cloud recognises this
          series.
        </p>
      )}

      <div className='flex items-center justify-end gap-2 pt-0.5'>
        <Button variant='ghost' size='sm' onClick={onCancel} disabled={busy} className='h-[30px] text-meta'>
          Cancel
        </Button>
        <Button
          size='sm'
          onClick={onSubmit}
          disabled={busy || !title.trim()}
          className='h-[30px] text-meta font-semibold'
        >
          {busy ? 'Saving…' : 'Save'}
        </Button>
      </div>
    </div>
  );
}
