/// Focus goes back to whatever opened a dialog — unless something has already
/// taken it while the dialog was closing, as the editor does after a pick from
/// the media library.
import { afterEach, describe, expect, it } from 'vitest';
import { renderHook } from '@testing-library/react';
import { useReturnFocus } from './useReturnFocus';

/// Radix dispatches the close event on the dialog's content element.
function closeFrom(dialog: HTMLElement, handler: (e: Event) => void) {
  dialog.addEventListener('close-auto-focus', handler);
  dialog.dispatchEvent(new Event('close-auto-focus', { cancelable: true }));
}

function setup() {
  const opener = document.createElement('button');
  const editor = document.createElement('textarea');
  const dialog = document.createElement('div');
  const inside = document.createElement('button');
  dialog.append(inside);
  document.body.append(opener, editor, dialog);
  const { result } = renderHook(() => useReturnFocus());
  opener.focus();
  result.current.onOpenAutoFocus();
  return { opener, editor, dialog, inside, focus: result.current };
}

afterEach(() => {
  document.body.innerHTML = '';
});

describe('useReturnFocus', () => {
  it('returns focus to the opener when it was left in the dialog', () => {
    const { opener, dialog, inside, focus } = setup();
    inside.focus();
    closeFrom(dialog, focus.onCloseAutoFocus);
    expect(opener).toHaveFocus();
  });

  it('returns focus to the opener when it fell to the page', () => {
    const { opener, dialog, inside, focus } = setup();
    inside.focus();
    inside.blur();
    closeFrom(dialog, focus.onCloseAutoFocus);
    expect(opener).toHaveFocus();
  });

  it('leaves focus where the work put it', () => {
    const { editor, dialog, focus } = setup();
    editor.focus();
    closeFrom(dialog, focus.onCloseAutoFocus);
    expect(editor).toHaveFocus();
  });
});
