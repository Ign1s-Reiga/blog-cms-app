/// The launch-time pull runs while the app may still be behind the login
/// screen, where it fails for want of credentials nobody has entered yet. That
/// failure is not news, and it must stay quiet even when it arrives after the
/// author has signed in and the shell has been mounted again.
import { describe, expect, it, vi } from 'vitest';
import { render } from '@testing-library/react';

const { pull, toastError } = vi.hoisted(() => ({ pull: vi.fn(), toastError: vi.fn() }));

vi.mock('@/lib/sync', () => ({ pullFromCloud: pull, pushToCloud: vi.fn() }));
vi.mock('sonner', () => ({ toast: { error: toastError, success: vi.fn() } }));

import { SyncActions } from './SyncActions';

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

    expect(toastError).not.toHaveBeenCalled();
  });
});
