/// An unused object deletes straight from its tile, with no dialog to show a
/// delete is running. A second tile's delete must go ahead meanwhile; only a
/// repeat press for the same object is dropped.
import { describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));

vi.mock('@tauri-apps/api/core', () => ({
  invoke,
  isTauri: () => true,
  convertFileSrc: (p: string) => `asset://localhost/${p}`,
}));
vi.mock('@tauri-apps/api/path', () => ({
  appDataDir: async () => '/data',
  join: async (...parts: string[]) => parts.join('/'),
}));
vi.mock('next/navigation', () => ({ useRouter: () => ({ push: vi.fn() }) }));

import MediaPage from './page';

describe('deleting unused media', () => {
  it('deletes a second object while the first is still going', async () => {
    const pending: (() => void)[] = [];
    invoke.mockImplementation(async (command: string) => {
      switch (command) {
        case 'list_media':
          return [
            { key: 'media/a.webp', name: 'a.webp', size: 10 },
            { key: 'media/b.webp', name: 'b.webp', size: 10 },
          ];
        case 'media_usage':
          // Reported with no posts: an object the survey leaves out counts as
          // unchecked, not unused, and goes through the warning instead.
          return {
            objects: [
              { key: 'media/a.webp', posts: [] },
              { key: 'media/b.webp', posts: [] },
            ],
            unchecked_posts: [],
          };
        case 'delete_media':
          return new Promise<void>((resolve) => pending.push(resolve));
        default:
          return undefined;
      }
    });
    render(<MediaPage />);

    await userEvent.click(await screen.findByRole('button', { name: 'Delete a.webp' }));
    await waitFor(() => expect(pending).toHaveLength(1));
    await userEvent.click(screen.getByRole('button', { name: 'Delete b.webp' }));
    await waitFor(() => expect(pending).toHaveLength(2));

    const deleted = invoke.mock.calls.filter(([command]) => command === 'delete_media').map(([, args]) => args.key);
    expect(deleted).toEqual(['media/a.webp', 'media/b.webp']);

    pending.forEach((resolve) => resolve());
    await waitFor(() => expect(screen.queryByRole('button', { name: /^Delete / })).not.toBeInTheDocument());
  });
});
