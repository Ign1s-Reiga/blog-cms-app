/// Enter in the tag field confirms the action, and it must hold to the same
/// rule as the confirm button: no tag, no run. The field is focused as the
/// dialog opens, so an Enter straight away is the easy mistake.
import { describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
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
});
