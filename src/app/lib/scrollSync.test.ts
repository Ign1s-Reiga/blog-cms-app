/// Tests for the split view's scroll sync.
///
/// jsdom lays nothing out, so the measuring half of the hook cannot run here.
/// What can is the part that decides which headings pair up and where a pane
/// lands between them — the part where a mistake scrolls to the wrong section.
import { describe, expect, it } from 'vitest';
import { buildAnchors, headingOffsets, mapScroll } from './scrollSync';

/// The text of each heading `headingOffsets` found, to keep failures readable.
function headings(md: string): string[] {
  return headingOffsets(md).map((at) => md.slice(at, md.indexOf('\n', at) === -1 ? undefined : md.indexOf('\n', at)));
}

describe('headingOffsets', () => {
  it('finds ATX headings, and not a hash without a space or an indented one', () => {
    const md = ['# One', 'text', '#tag', '  ## Two', '    ### code', '###', 'end'].join('\n');
    expect(headings(md)).toEqual(['# One', '  ## Two', '###']);
  });

  it('does not take a comment in fenced code for a heading', () => {
    const md = [
      '# Before',
      '```sh',
      '# install',
      '~~~',
      '# still code',
      '```',
      '# Between',
      '````md',
      '# inner',
      '```',
      '# still inner',
      '````',
      '# After',
    ].join('\n');
    expect(headings(md)).toEqual(['# Before', '# Between', '# After']);
  });

  it('places a setext heading at the start of its paragraph', () => {
    const md = ['intro', '', 'Long title', 'over two lines', '===', '', 'Sub', '---'].join('\n');
    expect(headings(md)).toEqual(['Long title', 'Sub']);
  });

  it('reads a dash line as a rule after a blank line or under a list item', () => {
    const md = ['para', '', '---', '', '- item', '---', '> quote', '---'].join('\n');
    expect(headingOffsets(md)).toEqual([]);
  });
});

describe('mapScroll', () => {
  it('lines up paired headings and interpolates between them', () => {
    // Editor headings at 100 and 300; the preview's at 400 and 500 (an image
    // between the top and the first heading).
    const anchors = buildAnchors([100, 300], [400, 500], 1000, 2000);
    expect(mapScroll(100, anchors)).toBe(400);
    expect(mapScroll(300, anchors)).toBe(500);
    expect(mapScroll(50, anchors)).toBe(200);
    expect(mapScroll(200, anchors)).toBe(450);
    expect(mapScroll(1000, anchors)).toBe(2000);
  });

  it('runs the same anchors backwards for the other pane', () => {
    const anchors = buildAnchors([400, 500], [100, 300], 2000, 1000);
    expect(mapScroll(450, anchors)).toBe(200);
  });

  it('falls back to proportion when the heading counts disagree', () => {
    const anchors = buildAnchors([100, 300], [400], 1000, 2000);
    expect(anchors).toEqual({ from: [0, 1000], to: [0, 2000] });
    expect(mapScroll(250, anchors)).toBe(500);
  });

  it('drops a heading that cannot reach the top of its pane, so the bottoms still meet', () => {
    // The second preview heading sits below the furthest the preview scrolls.
    const anchors = buildAnchors([100, 600], [200, 900], 1000, 800);
    expect(anchors).toEqual({ from: [0, 100, 1000], to: [0, 200, 800] });
    expect(mapScroll(1000, anchors)).toBe(800);
  });

  it('keeps a pane that cannot scroll at the top', () => {
    expect(mapScroll(0, buildAnchors([], [], 0, 500))).toBe(0);
    expect(mapScroll(300, buildAnchors([], [], 500, 0))).toBe(0);
  });
});
