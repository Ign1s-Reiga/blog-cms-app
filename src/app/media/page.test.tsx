/// Deletes on the Media page run side by side: an unused object deletes straight
/// from its tile, with no dialog to show it is busy. Each delete must keep to
/// its own object — not block another's, not close or lock another's
/// confirmation, not be undone by a reload that started before it, and not
/// leave an unnamed error behind.
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor, within } from '@testing-library/react';
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

const object = (name: string) => ({ key: `media/${name}`, name, size: 10 });
type Usage = {
  key: string;
  posts: { id: number; slug: string; title: string; trashed: boolean; published: boolean }[];
};
const usedBy = (name: string): Usage => ({
  key: `media/${name}`,
  posts: [{ id: 1, slug: 'dusk', title: 'Dusk', trashed: false, published: true }],
});
/// Reported with no posts: an object the survey leaves out counts as
/// unchecked, not unused, and goes through the warning instead.
const unused = (name: string): Usage => ({ key: `media/${name}`, posts: [] });

/// Answers by command. `delete_media` and, when asked, `list_media` are held
/// until the test lets them go, so they can be made to overlap.
function backend({
  listing = [object('a.webp'), object('b.webp')],
  usage = [unused('a.webp'), unused('b.webp')],
} = {}) {
  const deletes: { key: string; settle: (error?: string) => void }[] = [];
  const lists: ((rows: unknown[]) => void)[] = [];
  let holdLists = false;
  invoke.mockImplementation(async (command: string, args?: { key: string }) => {
    switch (command) {
      case 'list_media':
        return holdLists ? new Promise((resolve) => lists.push(resolve)) : listing;
      case 'media_usage':
        return { objects: usage, unchecked_posts: [] };
      case 'delete_media':
        return new Promise<void>((resolve, reject) =>
          deletes.push({ key: args!.key, settle: (error) => (error ? reject(error) : resolve()) }),
        );
      default:
        return undefined;
    }
  });
  return {
    deletes,
    lists,
    holdLists: () => {
      holdLists = true;
    },
  };
}

const tile = (name: string) => screen.queryByRole('button', { name: `Delete ${name}` });

describe('deleting media', () => {
  beforeEach(() => invoke.mockReset());

  it('deletes a second object while the first is still going', async () => {
    const { deletes } = backend();
    render(<MediaPage />);

    await userEvent.click((await screen.findByRole('button', { name: 'Delete a.webp' }))!);
    await userEvent.click(tile('b.webp')!);
    await waitFor(() => expect(deletes.map((d) => d.key)).toEqual(['media/a.webp', 'media/b.webp']));

    deletes.forEach((d) => d.settle());
    await waitFor(() => expect(tile('a.webp')).not.toBeInTheDocument());
    expect(tile('b.webp')).not.toBeInTheDocument();
  });

  it("leaves another object's confirmation open and usable", async () => {
    const { deletes } = backend({ usage: [unused('a.webp'), usedBy('b.webp')] });
    render(<MediaPage />);

    // A slow delete of an unused object, then the warning for a used one.
    await userEvent.click((await screen.findByRole('button', { name: 'Delete a.webp' }))!);
    await waitFor(() => expect(deletes).toHaveLength(1));
    await userEvent.click(tile('b.webp')!);
    const dialog = await screen.findByRole('alertdialog');

    expect(within(dialog).getByRole('button', { name: 'Delete anyway' })).toBeEnabled();
    expect(within(dialog).getByRole('button', { name: 'Keep it' })).toBeEnabled();

    deletes[0].settle();
    await waitFor(() => expect(tile('a.webp')).not.toBeInTheDocument());
    expect(screen.getByRole('alertdialog')).toBeInTheDocument();
  });

  it('does not bring back an object deleted while a reload was out', async () => {
    const { deletes, lists, holdLists } = backend();
    render(<MediaPage />);

    await userEvent.click((await screen.findByRole('button', { name: 'Delete a.webp' }))!);
    await waitFor(() => expect(deletes).toHaveLength(1));

    // An upload reloads the library; its listing is taken with a.webp still in R2.
    holdLists();
    await userEvent.click(screen.getByRole('button', { name: 'Upload' }));
    await waitFor(() => expect(lists).toHaveLength(1));

    deletes[0].settle();
    await waitFor(() => expect(tile('a.webp')).not.toBeInTheDocument());
    lists[0]([object('a.webp'), object('b.webp')]);

    await waitFor(() => expect(screen.getByRole('button', { name: 'Upload' })).toBeEnabled());
    expect(tile('a.webp')).not.toBeInTheDocument();
    expect(tile('b.webp')).toBeInTheDocument();
  });
});
