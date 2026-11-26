// Scroll sync between the Markdown source and its rendered preview.
//
// The two panes are not the same length — an image is one source line and a
// screenful of preview, a code block the other way round — so matching scroll
// *proportions* drifts as soon as a post has either. They are matched on
// headings instead: the Nth heading in the source sits beside the Nth heading
// in the preview, and positions between two headings are interpolated. The
// renderer (`@ign1s-reiga/marked-presets`) does not report which source line an
// element came from, and headings are the one thing both sides can find for
// themselves.
//
// When the counts disagree — a heading inside raw HTML, or a preview still
// rendering the previous keystroke — the anchors are dropped and the panes fall
// back to plain proportion rather than pairing the wrong headings.

import { useCallback, useEffect, useRef, type RefObject } from 'react';

const ATX = /^ {0,3}#{1,6}(?:[ \t]|$)/;
const SETEXT_UNDERLINE = /^ {0,3}(?:=+|-+)[ \t]*$/;
const FENCE = /^ {0,3}(`{3,}|~{3,})/;
/// A line that opens a quote, list or table. It ends any paragraph above it, so
/// a `---` under it is a rule or list content, not a setext underline.
const BLOCK_START = /^ {0,3}(?:[>|]|[-*+][ \t]|\d+[.)][ \t])/;
/// Indented code when no paragraph is open; a continuation line when one is.
const INDENTED = /^(?: {4}|\t)/;

/// Offsets in `md` of every top-level heading, ATX (`## Title`) or setext
/// (`Title` over `===`), in document order. Fenced code is skipped, so a
/// `# comment` in a shell block is not taken for one.
export function headingOffsets(md: string): number[] {
  const offsets: number[] = [];
  let fence: { char: string; len: number } | null = null;
  // Start of the paragraph the previous line belongs to, if it is one.
  let paragraph: number | null = null;
  let at = 0;

  for (const line of md.split('\n')) {
    const start = at;
    at += line.length + 1;

    if (fence) {
      const close = FENCE.exec(line);
      if (close && close[1][0] === fence.char && close[1].length >= fence.len && line.trim() === close[1]) {
        fence = null;
      }
      continue;
    }

    const open = FENCE.exec(line);
    if (open) {
      fence = { char: open[1][0], len: open[1].length };
      paragraph = null;
    } else if (line.trim() === '') {
      paragraph = null;
    } else if (ATX.test(line)) {
      offsets.push(start);
      paragraph = null;
    } else if (paragraph !== null && SETEXT_UNDERLINE.test(line)) {
      offsets.push(paragraph);
      paragraph = null;
    } else if (SETEXT_UNDERLINE.test(line) || BLOCK_START.test(line)) {
      paragraph = null;
    } else if (paragraph === null && !INDENTED.test(line)) {
      paragraph = start;
    }
  }
  return offsets;
}

export type Anchors = { from: number[]; to: number[] };

/// Pair two panes' heading positions into scroll anchors, bracketed by the top
/// and the bottom of each pane so both ends line up as well.
///
/// A heading too far down to be scrolled to the top of its pane is left out:
/// past that point only the bottoms can match. With unequal counts there is
/// nothing to pair, and only the ends are kept.
export function buildAnchors(from: number[], to: number[], fromMax: number, toMax: number): Anchors {
  const anchors: Anchors = { from: [0], to: [0] };
  if (from.length === to.length) {
    from.forEach((y, i) => {
      if (y > anchors.from.at(-1)! && to[i] > anchors.to.at(-1)! && y < fromMax && to[i] < toMax) {
        anchors.from.push(y);
        anchors.to.push(to[i]);
      }
    });
  }
  anchors.from.push(Math.max(fromMax, anchors.from.at(-1)!));
  anchors.to.push(Math.max(toMax, anchors.to.at(-1)!));
  return anchors;
}

/// Where the other pane should be when this one is scrolled to `y`.
export function mapScroll(y: number, { from, to }: Anchors): number {
  for (let i = 1; i < from.length; i++) {
    if (y <= from[i] || i === from.length - 1) {
      const span = from[i] - from[i - 1];
      const t = span <= 0 ? 0 : Math.min(1, Math.max(0, (y - from[i - 1]) / span));
      return to[i - 1] + t * (to[i] - to[i - 1]);
    }
  }
  return 0;
}

const MIRRORED: (keyof CSSStyleDeclaration & string)[] = [
  'fontFamily',
  'fontSize',
  'fontWeight',
  'fontStyle',
  'fontVariantLigatures',
  'fontFeatureSettings',
  'letterSpacing',
  'wordSpacing',
  'lineHeight',
  'textIndent',
  'textTransform',
  'tabSize',
  'whiteSpace',
  'overflowWrap',
  'wordBreak',
  'paddingTop',
  'paddingRight',
  'paddingBottom',
  'paddingLeft',
];

/// Where each of `offsets` sits in the textarea, in its scroll coordinates.
///
/// A source line is not one line high: it wraps. The textarea cannot say where
/// a character landed, so the text is laid out again in an off-screen copy with
/// the same font, padding and width, and a marker is read at each offset.
function textareaTops(el: HTMLTextAreaElement, offsets: number[]): number[] {
  const style = getComputedStyle(el);
  const mirror = document.createElement('div');
  for (const key of MIRRORED) mirror.style.setProperty(toKebab(key), style.getPropertyValue(toKebab(key)));
  Object.assign(mirror.style, {
    position: 'absolute',
    top: '0',
    left: '-99999px',
    visibility: 'hidden',
    border: '0',
    boxSizing: 'border-box',
    // clientWidth leaves out the scrollbar, which the text does not wrap under.
    width: `${el.clientWidth}px`,
  });

  const markers: HTMLSpanElement[] = [];
  let last = 0;
  for (const offset of offsets) {
    mirror.append(el.value.slice(last, offset));
    const marker = document.createElement('span');
    mirror.append(marker);
    markers.push(marker);
    last = offset;
  }
  mirror.append(el.value.slice(last));

  document.body.append(mirror);
  const tops = markers.map((m) => m.offsetTop);
  mirror.remove();
  return tops;
}

function toKebab(key: string): string {
  return key.replace(/[A-Z]/g, (c) => `-${c.toLowerCase()}`);
}

/// Top-level headings of the rendered preview, in its scroll coordinates. Only
/// direct children of the Markdown root: the footnotes section brings its own
/// `<h2>` that has no `#` in the source.
function previewTops(pane: HTMLElement): number[] {
  const root = pane.querySelector('.md-body');
  if (!root) return [];
  const paneTop = pane.getBoundingClientRect().top - pane.scrollTop;
  return Array.from(
    root.querySelectorAll(':scope > :is(h1, h2, h3, h4, h5, h6)'),
    (h) => h.getBoundingClientRect().top - paneTop,
  );
}

/// Keep the editor and preview panes scrolled to the same place while
/// `enabled`. Whichever pane the person scrolls leads; the other follows.
///
/// `html` is the rendered preview: when it changes the preview's layout does,
/// so it is brought back in line with the editor, which is where the typing
/// that changed it happened.
export function useScrollSync(
  enabled: boolean,
  editorRef: RefObject<HTMLTextAreaElement | null>,
  previewRef: RefObject<HTMLElement | null>,
  html: string,
): void {
  // Pane the person last scrolled, which a re-layout re-syncs from.
  const leader = useRef<'editor' | 'preview'>('editor');
  // A scroll this hook caused, so the pane it moved does not answer by moving
  // the first one back.
  const expected = useRef(new Map<Element, number>());
  // Measuring the textarea means laying out the whole body again, so it is
  // kept until the text or the width it wraps at changes.
  const measured = useRef<{ value: string; width: number; tops: number[] } | null>(null);

  const follow = useCallback(
    (from: 'editor' | 'preview') => {
      const editor = editorRef.current;
      const preview = previewRef.current;
      if (!editor || !preview) return;

      let cache = measured.current;
      if (!cache || cache.value !== editor.value || cache.width !== editor.clientWidth) {
        cache = {
          value: editor.value,
          width: editor.clientWidth,
          tops: textareaTops(editor, headingOffsets(editor.value)),
        };
        measured.current = cache;
      }

      const panes = { editor: [editor, cache.tops], preview: [preview, previewTops(preview)] } as const;
      const [source, sourceTops] = panes[from];
      const [target, targetTops] = panes[from === 'editor' ? 'preview' : 'editor'];
      const anchors = buildAnchors(
        sourceTops,
        targetTops,
        source.scrollHeight - source.clientHeight,
        target.scrollHeight - target.clientHeight,
      );
      const y = mapScroll(source.scrollTop, anchors);
      // Setting a pane to where it already is fires no scroll event, which would
      // leave the expectation behind to swallow the person's next scroll.
      if (Math.abs(target.scrollTop - y) > 1) {
        expected.current.set(target, y);
        target.scrollTop = y;
      }
    },
    [editorRef, previewRef],
  );

  useEffect(() => {
    const editor = editorRef.current;
    const preview = previewRef.current;
    if (!enabled || !editor || !preview) return;
    const pending = expected.current;

    const listen = (pane: HTMLElement, name: 'editor' | 'preview') => {
      const onScroll = () => {
        const echo = pending.get(pane);
        pending.delete(pane);
        if (echo !== undefined && Math.abs(pane.scrollTop - echo) <= 1) return;
        leader.current = name;
        follow(name);
      };
      pane.addEventListener('scroll', onScroll, { passive: true });
      return () => pane.removeEventListener('scroll', onScroll);
    };
    // An image arriving pushes everything under it down. `load` does not
    // bubble, so it is caught on the way down instead.
    const onLoad = () => follow(leader.current);

    const unlisten = [listen(editor, 'editor'), listen(preview, 'preview')];
    preview.addEventListener('load', onLoad, true);
    return () => {
      unlisten.forEach((off) => off());
      preview.removeEventListener('load', onLoad, true);
      pending.clear();
    };
  }, [enabled, editorRef, previewRef, follow]);

  useEffect(() => {
    if (!enabled) return;
    leader.current = 'editor';
    follow('editor');
  }, [enabled, html, follow]);
}
