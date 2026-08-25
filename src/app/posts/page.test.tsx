/// Tests for the posts list.
///
/// The frontend had no harness at all, so every fix to this file — the trash
/// selection leak, two states that could be set and never put down, a guard
/// reading a stale closure — went in with nothing able to catch a regression.
/// This is that harness, and the first of those bugs pinned.
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

const invoke = vi.fn();

/// The page reaches Tauri through `await import('@tauri-apps/api/core')` in each
/// handler rather than a top-level import, so the module mock has to answer for
/// every one of them.
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => invoke(...args),
  isTauri: () => true,
}));

vi.mock('next/navigation', () => ({
  useRouter: () => ({ push: vi.fn(), replace: vi.fn(), refresh: vi.fn() }),
  useSearchParams: () => new URLSearchParams(),
}));

import PostsPage from './page';

type Row = {
  id: number;
  slug: string;
  title: string;
  tags: string | null;
  published: boolean;
  created_at: number;
};

const row = (id: number, title: string): Row => ({
  id,
  slug: `post-${id}`,
  title,
  tags: '["rust"]',
  published: true,
  created_at: 0,
});

/// `loadPosts` asks for four things and tolerates three of them failing. Answer
/// them by name so a change in call order does not silently reorder fixtures.
const respond = (opts: { posts?: Row[]; trashed?: Row[] }) => {
  invoke.mockImplementation(async (command: string) => {
    switch (command) {
      case 'list_posts':
        return opts.posts ?? [];
      case 'list_trashed_posts':
        return (opts.trashed ?? []).map((p) => ({ ...p, trashed_at: 0 }));
      case 'list_sync_states':
        return [];
      case 'list_schedules':
        return [];
      default:
        throw new Error(`unexpected command: ${command}`);
    }
  });
};

describe('the posts list', () => {
  beforeEach(() => {
    invoke.mockReset();
  });

  it('shows the trash in its own tab', async () => {
    respond({ posts: [row(1, 'Live post')], trashed: [row(2, 'Binned post')] });
    render(<PostsPage />);

    await screen.findByText('Live post');
    expect(screen.queryByText('Binned post')).not.toBeInTheDocument();

    await userEvent.click(screen.getByRole('tab', { name: /Trash/ }));
    expect(await screen.findByText('Binned post')).toBeInTheDocument();
  });

  /// The regression from #123.
  ///
  /// `onScreen` was built from the library listing *and* the trash listing at
  /// once, while only one of them is ever rendered. A post ticked in the trash
  /// therefore stayed in `actionable` after a switch to All — invisible, still
  /// counted as selected, and offered Publish, Unpublish and the tag actions
  /// because `inTrash` had gone false with the tab.
  it('drops a trashed post from the selection when the tab changes', async () => {
    respond({ posts: [row(1, 'Live post')], trashed: [row(2, 'Binned post')] });
    render(<PostsPage />);

    await userEvent.click(await screen.findByRole('tab', { name: /Trash/ }));
    await userEvent.click(await screen.findByLabelText('Select Binned post'));
    expect(await screen.findByText('1 selected')).toBeInTheDocument();

    await userEvent.click(screen.getByRole('tab', { name: /^All/ }));

    await waitFor(() => {
      expect(screen.queryByText('1 selected')).not.toBeInTheDocument();
    });
    expect(screen.queryByText('Binned post')).not.toBeInTheDocument();
  });

  /// The same intersection from the other side: a post that is still on screen
  /// keeps its tick. Without this, gating `onScreen` on the tab could be
  /// "fixed" by clearing the selection on every tab change and still pass.
  it('keeps the selection for a post the tab change leaves on screen', async () => {
    respond({ posts: [row(1, 'Live post'), row(2, 'Another post')] });
    render(<PostsPage />);

    await userEvent.click(await screen.findByLabelText('Select Live post'));
    expect(await screen.findByText('1 selected')).toBeInTheDocument();

    await userEvent.click(screen.getByRole('tab', { name: /Published/ }));

    expect(await screen.findByText('1 selected')).toBeInTheDocument();
  });
});
