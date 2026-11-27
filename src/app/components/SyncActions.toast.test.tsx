/// The sync notices against the real sonner, not a mock of it.
///
/// sonner merges a toast into the one already showing under its id, so what a
/// notice ends up doing depends on sonner's behaviour as much as on the options
/// passed here — the mocked tests in SyncActions.test.tsx check the options,
/// and cannot see that a click's result inherited the standing notice's
/// `duration: Infinity`. This one renders the Toaster and runs the clock.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, render, screen } from '@testing-library/react';
import { Toaster } from 'sonner';

const { pull } = vi.hoisted(() => ({ pull: vi.fn() }));
vi.mock('@/lib/sync', async (original) => ({
  ...(await original<typeof import('@/lib/sync')>()),
  pullFromCloud: pull,
  pushToCloud: vi.fn(),
}));

import { SyncActions } from './SyncActions';

const notice = () => screen.queryByText('Could not pull posts from the cloud');

/// Let pending promises run and the clock move on, inside React's act.
const advance = (ms: number) => act(async () => void (await vi.advanceTimersByTimeAsync(ms)));

describe('the pull failure notice', () => {
  beforeEach(() => vi.useFakeTimers({ shouldAdvanceTime: false }));
  afterEach(() => vi.useRealTimers());

  it('stays while nobody asked, and fades once a click has been answered', async () => {
    pull.mockRejectedValue('401 Unauthorized');
    render(
      <>
        <Toaster />
        <SyncActions />
      </>,
    );

    // The launch-time failure: still there long after a click's result would go.
    await advance(30_000);
    expect(notice()).toBeInTheDocument();

    // A click that fails again replaces it, and that one fades.
    await act(async () => screen.getByRole('button', { name: 'Pull from cloud' }).click());
    await advance(100);
    expect(screen.getAllByText('Could not pull posts from the cloud')).toHaveLength(1);
    await advance(6_000);
    expect(notice()).not.toBeInTheDocument();
  });
});
