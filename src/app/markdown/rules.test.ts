/// The guide's language list is copied from marked-presets, which does not
/// export its own. If the renderer gains, drops or renames one, this fails
/// rather than the page telling authors a name works when it does not.
import { existsSync } from 'node:fs';
import { join } from 'node:path';
import { renderMarkdown } from '@ign1s-reiga/marked-presets';
import { describe, expect, it } from 'vitest';
import { LANGUAGES, SAMPLE_IMAGE, SECTIONS } from './rules';

const label = async (lang: string) =>
  (await renderMarkdown('```' + lang + '\nx\n```')).match(/<span class="code-lang">([^<]*)<\/span>/)?.[1];

describe('Markdown guide', () => {
  it('lists each highlighted language under the name the renderer gives it', async () => {
    for (const [id, name] of LANGUAGES) expect(await label(id), id).toBe(name);
  });

  it('is right that a long name falls back to plain text', async () => {
    expect(await label('rust')).toBe('Text');
  });

  it('ships the image the Images example shows', () => {
    expect(existsSync(join('public', SAMPLE_IMAGE))).toBe(true);
  });

  it('gives every example a unique id', () => {
    const ids = SECTIONS.flatMap((s) => s.rules.map((r) => r.id));
    expect(new Set(ids).size).toBe(ids.length);
  });
});
