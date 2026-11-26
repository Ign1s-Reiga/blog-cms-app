'use client';

import { Check, Copy } from 'lucide-react';
import { useTheme } from 'next-themes';
import { useEffect, useRef, useState } from 'react';
import { renderMarkdown } from '@ign1s-reiga/marked-presets';
import '@ign1s-reiga/marked-presets/styles';
import '@/components/markdown-theme.css';
import { SectionHeader } from '@/components/SectionHeader';
import { Button } from '@/components/ui/button';
import { LANGUAGES, SAMPLE_IMAGE, SECTIONS, type Rule } from './rules';

/// Text in backticks becomes inline code; the rest stays as written.
function Note({ text }: { text: string }) {
  return (
    <p className='text-meta leading-[1.6] text-muted-foreground'>
      {text.split('`').map((part, i) =>
        i % 2 === 1 ? (
          <code key={i} className='font-mono text-body bg-muted rounded-[3px] px-1 py-px'>
            {part}
          </code>
        ) : (
          part
        ),
      )}
    </p>
  );
}

function CopySource({ source, title }: { source: string; title: string }) {
  const [copied, setCopied] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);
  useEffect(() => () => clearTimeout(timer.current), []);

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(source);
    } catch {
      return; // no clipboard access: leave the label alone rather than claim it worked
    }
    setCopied(true);
    clearTimeout(timer.current);
    timer.current = setTimeout(() => setCopied(false), 1500);
  };

  return (
    <Button
      variant='ghost'
      size='xs'
      onClick={copy}
      aria-label={copied ? `Copied ${title}` : `Copy the ${title} example`}
      className='text-meta text-muted-foreground hover:text-foreground transition-colors active:scale-95'
    >
      {copied ? <Check className='text-success' /> : <Copy />}
      <span aria-live='polite'>{copied ? 'Copied' : 'Copy'}</span>
    </Button>
  );
}

function RuleCard({ rule, html, colorScheme }: { rule: Rule; html: string | undefined; colorScheme: string }) {
  return (
    <article aria-labelledby={`rule-${rule.id}`} className='rounded-lg border border-border bg-card overflow-hidden'>
      <div className='px-4 py-3 border-b border-border space-y-0.5'>
        <h3 id={`rule-${rule.id}`} className='text-ui font-semibold text-foreground'>
          {rule.title}
        </h3>
        {rule.note && <Note text={rule.note} />}
      </div>

      <div className='grid lg:grid-cols-2 divide-y lg:divide-y-0 lg:divide-x divide-border'>
        <div className='min-w-0 bg-muted/40'>
          <div className='flex items-center justify-between pl-4 pr-2 pt-2'>
            <span className='text-label font-bold uppercase tracking-[0.12em] text-muted-foreground'>Markdown</span>
            <CopySource source={rule.source} title={rule.title} />
          </div>
          <pre className='px-4 pt-1 pb-4 font-mono text-meta leading-[1.7] text-body whitespace-pre-wrap break-words'>
            {rule.source}
          </pre>
        </div>

        <div className='min-w-0 pl-8 pr-4 pt-2 pb-3'>
          <span className='block text-label font-bold uppercase tracking-[0.12em] text-muted-foreground'>Result</span>
          {html === undefined ? (
            <p className='mt-2 font-mono text-ui text-muted-foreground'>Rendering…</p>
          ) : (
            // The sources are this file's own constants, so the renderer's HTML
            // (with its inline copy handler and alert icons) goes in as is, as
            // it does in the editor's preview.
            <div
              className='md-body text-[14px] [&>:first-child]:mt-1'
              style={{ colorScheme }}
              dangerouslySetInnerHTML={{ __html: html }}
            />
          )}
        </div>
      </div>
    </article>
  );
}

export default function MarkdownGuidePage() {
  const { resolvedTheme } = useTheme();
  const colorScheme = resolvedTheme === 'dark' ? 'dark' : 'light';

  // Rendered once, through the editor's own renderer, so every result is what
  // a post would get. Shiki starts up on the first call; the rest reuse it.
  const [html, setHtml] = useState<Record<string, string>>({});
  useEffect(() => {
    let cancelled = false;
    const rules = SECTIONS.flatMap((s) => s.rules);
    void Promise.all(
      rules.map(async (rule) => {
        try {
          const out = await renderMarkdown(rule.source);
          return [rule.id, out.replaceAll(`src="${SAMPLE_IMAGE}"`, `src="/${SAMPLE_IMAGE}"`)] as const;
        } catch {
          return [rule.id, '<p class="md-error">This example failed to render.</p>'] as const;
        }
      }),
    ).then((entries) => {
      if (!cancelled) setHtml(Object.fromEntries(entries));
    });
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <main className='flex-1 overflow-y-auto p-6 scroll-pt-6'>
      <div className='max-w-[1040px] space-y-8'>
        <div className='space-y-3'>
          <div>
            <h1 className='text-title font-semibold tracking-tight text-balance text-foreground'>Markdown</h1>
            <p className='text-meta text-muted-foreground'>
              What the editor understands. Each example renders beside it exactly as it will in a post.
            </p>
          </div>

          <nav aria-label='Sections'>
            <ul className='flex flex-wrap gap-1.5'>
              {SECTIONS.map((s) => (
                <li key={s.id}>
                  <a
                    href={`#md-${s.id}`}
                    className='inline-flex h-7 items-center rounded-md border border-border bg-card px-2.5 text-meta font-medium text-body transition-colors hover:bg-muted hover:text-foreground active:scale-95'
                  >
                    {s.title}
                  </a>
                </li>
              ))}
            </ul>
          </nav>
        </div>

        {SECTIONS.map((section) => (
          <section key={section.id} id={`md-${section.id}`} aria-label={section.title}>
            <SectionHeader>{section.title}</SectionHeader>

            {section.id === 'code' && (
              <div className='mb-4 rounded-lg border border-border bg-card px-4 py-3 space-y-2'>
                <h3 className='text-ui font-semibold text-foreground'>Highlighted languages</h3>
                <Note text='Write the name exactly as shown. `rs` is highlighted; `rust` is plain text.' />
                <ul className='flex flex-wrap gap-1.5'>
                  {LANGUAGES.map(([id, name]) => (
                    <li
                      key={id}
                      className='inline-flex items-baseline gap-1.5 rounded-md border border-border px-2 py-0.5 text-meta'
                    >
                      <code className='font-mono font-semibold text-foreground'>{id}</code>
                      <span className='text-muted-foreground'>{name}</span>
                    </li>
                  ))}
                </ul>
              </div>
            )}

            <div className='space-y-4'>
              {section.rules.map((rule) => (
                <RuleCard key={rule.id} rule={rule} html={html[rule.id]} colorScheme={colorScheme} />
              ))}
            </div>
          </section>
        ))}
      </div>
    </main>
  );
}
