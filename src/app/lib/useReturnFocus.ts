import { useRef } from 'react';

/// Focus handling for a Radix dialog that is opened from state rather than from
/// a `DialogTrigger`.
///
/// On close, Radix sends focus to the dialog's trigger, and with no trigger it
/// sends it nowhere: focus falls to `<body>`, and the next Tab starts again
/// from the top of the app. Spread these onto the dialog's content to send it
/// back to whatever had it when the dialog opened instead.
///
/// The opener is read in `onOpenAutoFocus`, which Radix fires before it moves
/// focus into the dialog, so `document.activeElement` is still the button that
/// was pressed. If that element has gone by the time the dialog closes — a
/// deleted row takes its button with it — there is nothing to return to, and
/// focus is left where it is.
export function useReturnFocus() {
  const opener = useRef<HTMLElement | null>(null);

  return {
    onOpenAutoFocus: () => {
      opener.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    },
    onCloseAutoFocus: (e: Event) => {
      e.preventDefault();
      const el = opener.current;
      opener.current = null;
      if (el?.isConnected) el.focus();
    },
  };
}
