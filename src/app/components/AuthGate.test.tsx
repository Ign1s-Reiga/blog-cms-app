/// Signing in remounts the app, and the header's launch-time pull runs again
/// with the new credentials. That pull is the one report of a bad token: the
/// gate must not run a second pull of its own, which raced it and showed the
/// same failure twice in the same corner.
import { describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

const { invoke, pull, toastError } = vi.hoisted(() => ({
  invoke: vi.fn(),
  pull: vi.fn(),
  toastError: vi.fn(),
}));

vi.mock('@tauri-apps/api/core', () => ({ invoke, isTauri: () => true }));
vi.mock('@/lib/sync', async (original) => ({
  ...(await original<typeof import('@/lib/sync')>()),
  pullFromCloud: pull,
  pushToCloud: vi.fn(),
}));
vi.mock('sonner', () => ({ toast: { error: toastError, success: vi.fn() } }));

import { AuthGate } from './AuthGate';
import { SyncActions } from './SyncActions';

describe('signing in', () => {
  it('reports a bad token once, through the pull the remounted app makes', async () => {
    invoke.mockImplementation(async (command: string) =>
      command === 'session_status' ? { authenticated: false } : undefined,
    );
    pull.mockRejectedValue('401 Unauthorized');

    render(
      <AuthGate>
        <SyncActions />
      </AuthGate>,
    );

    // Values that pass the form's own checks: the D1 id must be a UUID and
    // the public URL an origin.
    for (const [placeholder, value] of [
      ['Cloudflare account id', 'acct'],
      ['R2 + D1 edit token', 'bad-token'],
      ['bucket name', 'blog'],
      ['database id', '00000000-0000-4000-8000-000000000000'],
      ['https://cdn.example.com', 'https://cdn.example.com'],
    ]) {
      await userEvent.type(await screen.findByPlaceholderText(placeholder), value);
    }
    await userEvent.click(screen.getByRole('button', { name: 'Connect' }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('save_credentials', expect.anything()));

    // One pull behind the login, which stays quiet, and one after it.
    await waitFor(() => expect(pull).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(toastError).toHaveBeenCalled());
    // Let anything still pending settle, so a second report arriving a tick
    // later — the race this guards against — is counted too.
    await new Promise((r) => setTimeout(r, 50));
    expect(toastError).toHaveBeenCalledTimes(1);
    // Nobody clicked for this pull, so its report stays until dismissed rather
    // than fading and leaving an empty library to explain itself.
    expect(toastError).toHaveBeenCalledWith(expect.any(String), expect.objectContaining({ duration: Infinity }));
    expect(screen.queryByText(/Could not read this account/)).not.toBeInTheDocument();
  });
});
