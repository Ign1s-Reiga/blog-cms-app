/// `cn` must treat the app's type roles as font sizes. If it read them as
/// colours, a sized Button would lose its variant's text colour, which leaves
/// dark text on a dark button. That is how this was found.
import { describe, expect, it } from 'vitest';
import { cn } from './utils';

describe('cn', () => {
  it('keeps a text colour when a type role is added after it', () => {
    expect(cn('text-primary-foreground', 'text-ui')).toBe('text-primary-foreground text-ui');
  });

  it('lets a later type role replace an earlier one', () => {
    expect(cn('text-meta', 'text-heading')).toBe('text-heading');
  });
});
