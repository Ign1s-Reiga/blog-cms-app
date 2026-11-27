/// Deletes on the Media page run side by side: an unused object deletes straight
/// from its tile, with no dialog to show it is busy. Each delete must keep to
/// its own object — not block another's, not close or lock another's
/// confirmation, not be undone by a reload that started before it, and not
/// leave an unnamed error behind.
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

const { invoke, join } = vi.hoisted(() => ({
  invoke: vi.fn(),
  /// Counted: the last thing a reload awaits per object, so a test can tell
  /// when a held reload has run all the way to applying its listing.
  join: vi.fn(async (...parts: string[]) => parts.join('/')),
}));

vi.mock('@tauri-apps/api/core', () => ({
  invoke,
  isTauri: () => true,
  convertFileSrc: (p: string) => `asset://localhost/${p}`,
}));
vi.mock('@tauri-apps/api/path', () => ({
  appDataDir: async () => '/data',
  join,
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
/// until the test lets them go, so they can be made to overlap. The listing
/// can be changed between reloads, as R2 changes under the page.
function backend({
  listing = [object('a.webp'), object('b.webp')],
  usage = [unused('a.webp'), unused('b.webp')],
  holdFirstList = false,
} = {}) {
  const deletes: { key: string; settle: (error?: string) => void }[] = [];
  const lists: ((rows: unknown[]) => void)[] = [];
  let holdLists = holdFirstList;
  let failLists = false;
  let current = listing;
  invoke.mockImplementation(async (command: string, args?: { key: string }) => {
    switch (command) {
      case 'list_media':
        if (failLists) throw 'network unreachable';
        return holdLists ? new Promise((resolve) => lists.push(resolve)) : current;
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
    holdLists: (on = true) => {
      holdLists = on;
    },
    setListing: (rows: ReturnType<typeof object>[]) => {
      current = rows;
    },
    failLists: (on = true) => {
      failLists = on;
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

  it('names the object a delete failed for, and clears that once it goes', async () => {
    const { deletes } = backend();
    render(<MediaPage />);

    await userEvent.click((await screen.findByRole('button', { name: 'Delete a.webp' }))!);
    await waitFor(() => expect(deletes).toHaveLength(1));
    deletes[0].settle('R2 timed out');
    expect(await screen.findByRole('alert')).toHaveTextContent('Could not delete a.webp: R2 timed out');

    await userEvent.click(tile('a.webp')!);
    await waitFor(() => expect(deletes).toHaveLength(2));
    deletes[1].settle();
    await waitFor(() => expect(screen.queryByRole('alert')).not.toBeInTheDocument());
  });

  it('shows a tile as busy while its own delete runs', async () => {
    const { deletes } = backend();
    render(<MediaPage />);

    await userEvent.click((await screen.findByRole('button', { name: 'Delete a.webp' }))!);
    await waitFor(() => expect(deletes).toHaveLength(1));

    // Marked busy without `disabled`, which would drop its keyboard focus.
    expect(tile('a.webp')).toHaveAttribute('aria-disabled', 'true');
    expect(tile('a.webp')).toHaveAttribute('aria-busy', 'true');
    expect(tile('a.webp')).toBeEnabled();
    expect(tile('b.webp')).not.toHaveAttribute('aria-disabled');

    // A press while it is busy sends nothing more.
    await userEvent.click(tile('a.webp')!);
    expect(deletes).toHaveLength(1);
  });

  it('keeps a notice per failed object, each dismissable on its own', async () => {
    const { deletes } = backend();
    render(<MediaPage />);

    await userEvent.click((await screen.findByRole('button', { name: 'Delete a.webp' }))!);
    await userEvent.click(tile('b.webp')!);
    await waitFor(() => expect(deletes).toHaveLength(2));
    deletes[0].settle('R2 timed out');
    deletes[1].settle('R2 refused');

    const alert = await screen.findByRole('alert');
    await waitFor(() => expect(alert).toHaveTextContent('Could not delete b.webp: R2 refused'));
    expect(alert).toHaveTextContent('Could not delete a.webp: R2 timed out');

    // b goes through on a retry; a's failure is still reported.
    await userEvent.click(tile('b.webp')!);
    await waitFor(() => expect(deletes).toHaveLength(3));
    deletes[2].settle();
    await waitFor(() => expect(screen.getByRole('alert')).not.toHaveTextContent('b.webp'));
    expect(screen.getByRole('alert')).toHaveTextContent('Could not delete a.webp');

    await userEvent.click(screen.getByRole('button', { name: /^Dismiss: Could not delete a\.webp/ }));
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it("drops a failed delete's notice once a reload no longer lists the object", async () => {
    const { deletes, setListing } = backend();
    render(<MediaPage />);

    await userEvent.click((await screen.findByRole('button', { name: 'Delete a.webp' }))!);
    await waitFor(() => expect(deletes).toHaveLength(1));
    deletes[0].settle('R2 timed out');
    expect(await screen.findByRole('alert')).toHaveTextContent('Could not delete a.webp');

    // The delete went through after all, and the next listing says so.
    setListing([object('b.webp')]);
    await userEvent.click(screen.getByRole('button', { name: 'Upload' }));

    await waitFor(() => expect(tile('a.webp')).not.toBeInTheDocument());
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('lets only the newest reload set the library', async () => {
    const { lists, holdLists, setListing } = backend({ holdFirstList: true });
    render(<MediaPage />);
    await waitFor(() => expect(lists).toHaveLength(1));

    // An upload's reload overtakes the first load and finds the new object.
    holdLists(false);
    setListing([object('a.webp'), object('b.webp'), object('c.webp')]);
    await userEvent.click(screen.getByRole('button', { name: 'Upload' }));
    expect(await screen.findByRole('button', { name: 'Delete c.webp' })).toBeInTheDocument();

    // The first load finally lands, with a listing from before the upload. Its
    // last await is a `join` per object; once both have run, one more task
    // turn lets it reach the point where it would apply its listing.
    const joined = join.mock.calls.length;
    lists[0]([object('a.webp'), object('b.webp')]);
    await waitFor(() => expect(join).toHaveBeenCalledTimes(joined + 2));
    await new Promise((r) => setTimeout(r, 0));
    expect(tile('c.webp')).toBeInTheDocument();
  });

  it('keeps the library on screen while a newer reload is still out', async () => {
    const { lists } = backend({ holdFirstList: true });
    render(<MediaPage />);
    await waitFor(() => expect(lists).toHaveLength(1));

    // An upload's reload starts and is held too; then the first load lands.
    await userEvent.click(screen.getByRole('button', { name: 'Upload' }));
    await waitFor(() => expect(lists).toHaveLength(2));
    lists[0]([object('a.webp'), object('b.webp')]);

    expect(await screen.findByRole('button', { name: 'Delete a.webp' })).toBeInTheDocument();
    expect(screen.queryByText(/No media yet/)).not.toBeInTheDocument();
  });

  it("shows an older reload's listing when the newer one failed", async () => {
    const { lists, holdLists, failLists } = backend({ holdFirstList: true });
    render(<MediaPage />);
    await waitFor(() => expect(lists).toHaveLength(1));

    holdLists(false);
    failLists();
    await userEvent.click(screen.getByRole('button', { name: 'Upload' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('network unreachable');
    // The first load is still out, and may yet bring the library: until it
    // settles, the page is loading, not empty.
    expect(screen.getByText('Loading media…')).toBeInTheDocument();
    expect(screen.queryByText(/No media yet/)).not.toBeInTheDocument();

    lists[0]([object('a.webp'), object('b.webp')]);
    expect(await screen.findByRole('button', { name: 'Delete a.webp' })).toBeInTheDocument();
    // The failure it answers is the newer reload's, and it still stands.
    expect(screen.getByRole('alert')).toHaveTextContent('network unreachable');
  });

  it('moves focus to the next notice, then to Upload, as notices are dismissed', async () => {
    const { deletes } = backend();
    render(<MediaPage />);

    await userEvent.click((await screen.findByRole('button', { name: 'Delete a.webp' }))!);
    await userEvent.click(tile('b.webp')!);
    await waitFor(() => expect(deletes).toHaveLength(2));
    deletes[0].settle('R2 timed out');
    deletes[1].settle('R2 refused');
    const dismissA = await screen.findByRole('button', { name: /^Dismiss: Could not delete a\.webp/ });
    const dismissB = await screen.findByRole('button', { name: /^Dismiss: Could not delete b\.webp/ });

    dismissA.focus();
    await userEvent.keyboard('{Enter}');
    await waitFor(() => expect(dismissB).toHaveFocus());

    await userEvent.keyboard('{Enter}');
    await waitFor(() => expect(screen.getByRole('button', { name: 'Upload' })).toHaveFocus());
  });
});
