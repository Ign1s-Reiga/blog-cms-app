'use client';

import { useCallback, useEffect, useState } from 'react';
import { FileText, Loader2, X } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';

/// Mirrors `ImportProposal` in `src-tauri/src/commands/local_db.rs`.
///
/// The file itself never crosses over — the backend holds the body it read and
/// hands out `token`, which is the only thing it will import. Nothing here can
/// name a different file, which is the point.
export type ImportProposal = {
  token: string;
  file_name: string;
  title: string;
  tags: string;
  excerpt: string;
  /// Seconds. Null dates the post to the moment it is imported.
  created_at: number | null;
  /// Which of the fields above the document stated for itself, rather than the
  /// app falling back to the file name.
  from_file: string[];
  /// Front matter keys nothing read.
  ignored: string[];
};

/// Said plainly, because "ignored" alone reads as a fault in the file rather
/// than a limit of what the app has anywhere to put.
const IGNORED_NOTE: Record<string, string> = {
  published: 'an import always lands as a draft',
  date: 'not in a date format this reads',
};

function formatDate(seconds: number): string {
  return new Date(seconds * 1000).toLocaleDateString(undefined, {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
  });
}

/// Confirm what an imported file proposes for itself before anything is created.
///
/// The values are the file's suggestion and nothing more: front matter is not
/// authoritative here, because the blog reads a post's metadata from D1. What it
/// saves is the retyping.
export function ImportDialog({
  proposal,
  onCancel,
  onImported,
}: {
  proposal: ImportProposal | null;
  onCancel: () => void;
  onImported: (title: string) => void;
}) {
  const [title, setTitle] = useState('');
  const [tags, setTags] = useState('');
  const [excerpt, setExcerpt] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Seed the fields from each new proposal, so picking a second file refills
  // the form rather than leaving the first file's values sitting in it.
  useEffect(() => {
    if (!proposal) return;
    setTitle(proposal.title);
    setTags(proposal.tags);
    setExcerpt(proposal.excerpt);
    setError(null);
    setBusy(false);
  }, [proposal]);

  const cancel = useCallback(async () => {
    if (busy) return;
    if (proposal) {
      const { invoke, isTauri } = await import('@tauri-apps/api/core');
      // Let the backend drop the body it is holding. A failure here costs one
      // staged document until the next pick displaces it, which is not worth
      // keeping the dialog open over.
      if (isTauri()) await invoke('cancel_import', { token: proposal.token }).catch(() => {});
    }
    onCancel();
  }, [busy, proposal, onCancel]);

  // Escape closes it, the same way the media dialogs do.
  useEffect(() => {
    if (!proposal) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') void cancel();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [proposal, cancel]);

  if (!proposal) return null;

  const confirm = async () => {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      const { invoke } = await import('@tauri-apps/api/core');
      const imported = await invoke<string>('commit_import', {
        token: proposal.token,
        title,
        tags,
        excerpt,
        createdAt: proposal.created_at,
      });
      onImported(imported);
    } catch (err) {
      setError(String(err));
      setBusy(false);
    }
  };

  const stated = proposal.from_file;
  const fromFile = (field: string) => stated.includes(field);

  return (
    <div
      role='dialog'
      aria-modal='true'
      aria-label='Confirm import'
      className='fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-6'
      onPointerDown={(e) => {
        if (e.target === e.currentTarget) void cancel();
      }}
    >
      <div className='flex max-h-[80vh] w-full max-w-[520px] flex-col rounded-[8px] border border-zinc-200 dark:border-white/[0.08] bg-white dark:bg-[#161616]'>
        <div className='flex items-start justify-between gap-3 border-b border-zinc-100 dark:border-white/[0.05] px-4 py-3'>
          <div className='min-w-0'>
            <h2 className='flex items-center gap-2 text-[13px] font-semibold text-zinc-800 dark:text-zinc-200'>
              <FileText size={14} strokeWidth={2} className='text-zinc-400 dark:text-zinc-500' />
              Import as a draft
            </h2>
            <p className='mt-0.5 truncate font-mono text-[11px] text-zinc-400 dark:text-zinc-600'>
              {proposal.file_name}
            </p>
          </div>
          <Button
            variant='ghost'
            size='icon'
            aria-label='Close'
            onClick={() => void cancel()}
            className='size-[26px] shrink-0 rounded-[5px] text-zinc-400 dark:text-zinc-500'
          >
            <X size={13} strokeWidth={2} />
          </Button>
        </div>

        <div className='min-h-0 flex-1 space-y-4 overflow-y-auto px-4 py-3'>
          <p className='text-[11px] leading-[1.5] text-zinc-500 dark:text-zinc-600'>
            {stated.length > 0
              ? 'Filled in from the file’s front matter. The blog reads a published post’s metadata from D1, not from the file, so these are a starting point — change anything that is wrong.'
              : 'This file states no metadata of its own, so the title comes from its name. Fill in the rest here or later in the editor.'}
          </p>

          <Labelled label='Title' fromFile={fromFile('title')}>
            <Input
              value={title}
              onChange={(e) => setTitle(e.target.value)}
              placeholder='Post title'
              className='h-[32px] text-[12px]'
            />
          </Labelled>

          <Labelled label='Tags' fromFile={fromFile('tags')}>
            <Input
              value={tags}
              onChange={(e) => setTags(e.target.value)}
              placeholder='Comma-separated'
              className='h-[32px] text-[12px]'
            />
          </Labelled>

          <Labelled label='Excerpt' fromFile={fromFile('excerpt')}>
            <Input
              value={excerpt}
              onChange={(e) => setExcerpt(e.target.value)}
              placeholder='Optional summary'
              className='h-[32px] text-[12px]'
            />
          </Labelled>

          {proposal.created_at !== null && (
            <p className='text-[11px] text-zinc-500 dark:text-zinc-600'>
              Dated{' '}
              <span className='font-medium text-zinc-700 dark:text-zinc-300'>{formatDate(proposal.created_at)}</span>{' '}
              from the file. Posts are listed by this date.
            </p>
          )}

          {/* Naming what was passed over is the difference between "you gave no
              date" and "your date was not read". */}
          {proposal.ignored.length > 0 && (
            <p className='text-[11px] leading-[1.5] text-zinc-500 dark:text-zinc-600'>
              Not read from the file:{' '}
              {proposal.ignored.map((key, i) => (
                <span key={key}>
                  {i > 0 && ', '}
                  <span className='font-mono text-zinc-600 dark:text-zinc-400'>{key}</span>
                  {IGNORED_NOTE[key] && ` (${IGNORED_NOTE[key]})`}
                </span>
              ))}
              .
            </p>
          )}

          {error && <p className='text-[12px] text-red-600 dark:text-red-400'>{error}</p>}
        </div>

        <div className='flex items-center justify-end gap-2 border-t border-zinc-100 dark:border-white/[0.05] px-4 py-3'>
          <Button
            variant='ghost'
            size='sm'
            onClick={() => void cancel()}
            disabled={busy}
            className='h-[30px] text-[12px]'
          >
            Cancel
          </Button>
          <Button
            size='sm'
            onClick={() => void confirm()}
            disabled={busy}
            className='h-[30px] gap-1.5 text-[12px] font-semibold'
          >
            {busy && <Loader2 size={12} strokeWidth={2} className='animate-spin' />}
            {busy ? 'Importing…' : 'Import'}
          </Button>
        </div>
      </div>
    </div>
  );
}

/// A labelled field, marked when the value came from the document rather than
/// from the app guessing.
function Labelled({ label, fromFile, children }: { label: string; fromFile: boolean; children: React.ReactNode }) {
  return (
    <div className='space-y-1.5'>
      <div className='flex items-baseline gap-2'>
        <label className='block text-[12px] font-medium text-zinc-700 dark:text-zinc-300'>{label}</label>
        {fromFile && (
          <span className='text-[10px] font-semibold uppercase tracking-[0.08em] text-zinc-400 dark:text-zinc-600'>
            from file
          </span>
        )}
      </div>
      {children}
    </div>
  );
}
