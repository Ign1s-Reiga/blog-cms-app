/// Enter in the tag field confirms the action, and it must hold to the same
/// rule as the confirm button: no tag, no run. The field is focused as the
/// dialog opens, so an Enter straight away is the easy mistake.
import { describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { BulkActions } from './BulkActions';

const ok = { done: 2, failed: [], skipped: [] };

describe('bulk tag actions', () => {
  it('does nothing on Enter with no tag, and runs once one is typed', async () => {
    const onRun = vi.fn(async () => ok);
    render(<BulkActions selected={[1, 2]} onClear={() => {}} onRun={onRun} inTrash={false} />);

    await userEvent.click(screen.getByRole('button', { name: 'Add tag' }));
    const field = await screen.findByRole('textbox', { name: 'Tag name' });
    expect(field).toHaveFocus();

    await userEvent.keyboard('{Enter}');
    await userEvent.type(field, '   {Enter}');
    expect(onRun).not.toHaveBeenCalled();

    await userEvent.type(field, 'rust{Enter}');
    expect(onRun).toHaveBeenCalledWith('addTag', 'rust');
  });

  /// Opened from state, with no trigger, so it is on this component to hand
  /// focus back — Radix alone would drop it on the page body.
  it('gives focus back to the button that opened it', async () => {
    render(<BulkActions selected={[1, 2]} onClear={() => {}} onRun={vi.fn(async () => ok)} inTrash={false} />);

    const opener = screen.getByRole('button', { name: 'Publish' });
    await userEvent.click(opener);
    await screen.findByRole('alertdialog');
    await userEvent.keyboard('{Escape}');

    await waitFor(() => expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument());
    expect(opener).toHaveFocus();
  });

  it('says so in the dialog when the run fails as a whole', async () => {
    const onRun = vi.fn(async () => {
      throw 'database is locked';
    });
    render(<BulkActions selected={[1, 2]} onClear={() => {}} onRun={onRun} inTrash={false} />);

    await userEvent.click(screen.getByRole('button', { name: 'Publish' }));
    const dialog = await screen.findByRole('alertdialog');
    await userEvent.click(within(dialog).getByRole('button', { name: 'Publish' }));

    expect(await within(dialog).findByRole('alert')).toHaveTextContent('database is locked');
    expect(dialog).toBeInTheDocument();
  });

  /// A live region is read out reliably only if it is already in the page when
  /// its text arrives, so it is there, empty, before the run.
  it('announces the result through a status region that was waiting for it', async () => {
    render(<BulkActions selected={[1, 2]} onClear={() => {}} onRun={vi.fn(async () => ok)} inTrash={false} />);

    const status = screen.getByRole('status');
    expect(status).toBeEmptyDOMElement();

    await userEvent.click(screen.getByRole('button', { name: 'Publish' }));
    const dialog = await screen.findByRole('alertdialog');
    await userEvent.click(within(dialog).getByRole('button', { name: 'Publish' }));

    await waitFor(() => expect(status).toHaveTextContent('Publish: 2 done.'));
  });
});
