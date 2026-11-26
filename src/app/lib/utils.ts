import { clsx, type ClassValue } from 'clsx';
import { extendTailwindMerge } from 'tailwind-merge';

/// tailwind-merge only knows Tailwind's stock scale, and reads any other
/// `text-*` as a colour. Without this, `text-ui` on a Button would be taken for
/// a colour and silently drop the variant's `text-primary-foreground`, leaving
/// dark text on a dark button. Keep in step with the `--text-*` roles in
/// globals.css.
const twMerge = extendTailwindMerge({
  extend: {
    classGroups: {
      'font-size': [{ text: ['label', 'meta', 'ui', 'heading', 'title'] }],
    },
  },
});

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}
