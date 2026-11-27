/// The launch-time pull runs while the app may still be behind the login
/// screen, where it fails for want of credentials nobody has entered yet. That
/// failure is not news, and it must stay quiet even when it arrives after the
/// author has signed in and the shell has been mounted again.
import { describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

const { pull, toast } = vi.hoisted(() => ({
  pull: vi.fn(),
  toast: Object.assign(vi.fn(), { error: vi.fn(), success: vi.fn(), dismiss: vi.fn() }),
}));

vi.mock('@/lib/sync', async (original) => ({
  ...(await original<typeof import('@/lib/sync')>()),
  pullFromCloud: pull,
  pushToCloud: vi.fn(),
}));
vi.mock('sonner', () => ({ toast }));

import { SyncActions } from './SyncActions';
import { SYNC_UNAVAILABLE } from '@/lib/sync';

describe('the launch-time pull', () => {
  it('stays quiet about a failure that started behind the login, even once that shell is gone', async () => {
    let fail: (reason: unknown) => void = () => {};
    pull.mockImplementation(() => new Promise<void>((_, reject) => (fail = reject)));

    const { unmount } = render(
      <div inert>
        <SyncActions />
      </div>,
    );
    expect(pull).toHaveBeenCalled();

    // Signing in remounts the shell before the pull gives up.
    unmount();
    fail('no credentials');
    await new Promise((r) => setTimeout(r, 0));

    expect(toast.error).not.toHaveBeenCalled();
  });

  /// Outside the desktop app there is no backend and the helper does nothing.
  /// A click says that, rather than announcing a pull that never happened.
  it('says sync is unavailable instead of claiming a pull it did not make', async () => {
    pull.mockResolvedValue(false);
    render(<SyncActions />);

    await userEvent.click(screen.getByRole('button', { name: 'Pull from cloud' }));

    expect(toast).toHaveBeenCalledWith(SYNC_UNAVAILABLE);
    expect(toast.success).not.toHaveBeenCalled();
  });

  /// The launch-time failure stays until dismissed, so it must not stack on a
  /// repeat or outlive the problem: it has one id, and a pull that works takes
  /// it away.
  it('keeps one standing failure notice, and clears it once a pull works', async () => {
    pull.mockRejectedValueOnce('401 Unauthorized').mockResolvedValue(true);
    render(<SyncActions />);

    await vi.waitFor(() => expect(toast.error).toHaveBeenCalled());
    const [, options] = toast.error.mock.calls[0];
    expect(options).toMatchObject({ id: expect.any(String), duration: Infinity });

    await userEvent.click(screen.getByRole('button', { name: 'Pull from cloud' }));
    expect(toast.dismiss).toHaveBeenCalledWith(options.id);
  });
});
