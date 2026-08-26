/// Tests for the editor's thumbnail control, its pre-publish check, and its
/// slug field.
///
/// The command behind it shipped registered and unreachable, so the thing worth
/// pinning is that it is reachable — and that the two states around it are
/// right: a post with no slug yet has nowhere to put a thumbnail, and a
/// dismissed file dialog is not a failure.
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

const invoke = vi.fn();

/// The editor reads `window.location.search` directly rather than through
/// `useSearchParams`, so opening a post means moving jsdom's URL.
const openPost = (id: number) => window.history.replaceState({}, '', `/posts/edit?id=${id}`);
const openNewPost = () => window.history.replaceState({}, '', '/posts/new');

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => invoke(...args),
  isTauri: () => true,
  convertFileSrc: (p: string) => `asset://localhost/${p}`,
}));

vi.mock('@tauri-apps/api/path', () => ({
  appDataDir: async () => '/data',
  join: async (...parts: string[]) => parts.join('/'),
}));

/// The editor subscribes to the webview's drag-and-drop events on mount, which
/// reaches for a window that does not exist outside the shell.
vi.mock('@tauri-apps/api/webview', () => ({
  getCurrentWebview: () => ({ onDragDropEvent: async () => () => {} }),
}));

vi.mock('next/navigation', () => ({
  useRouter: () => ({ push: vi.fn(), replace: vi.fn(), back: vi.fn() }),
  useSearchParams: () => new URLSearchParams(),
}));

import { PostEditor } from './PostEditor';

/// The editor asks for several things on mount and tolerates most of them
/// failing. Answer by name; anything unnamed is an empty list rather than a
/// throw, so a new call added later does not fail these tests for the wrong
/// reason.
/// A failure, as the IPC boundary actually delivers one.
///
/// `AppError` serializes to a bare string, and Tauri rejects with that value —
/// not with an `Error` wrapping it. The distinction decides these tests:
/// `String(err)` on a rejected string is the string, and on an `Error` it is
/// `"Error: ..."`, so a fixture that threw an `Error` would never match the
/// `=== 'cancelled'` compare the code makes and would fail it for a reason the
/// real app does not have.
const rejects = (message: string) => ({ __reject: message });

const backend = (over: Record<string, unknown> = {}) => {
  invoke.mockImplementation(async (command: string) => {
    if (command in over) {
      const value = over[command];
      if (value !== null && typeof value === 'object' && '__reject' in value) {
        throw (value as { __reject: string }).__reject;
      }
      return typeof value === 'function' ? (value as () => unknown)() : value;
    }
    switch (command) {
      case 'get_post':
        return {
          title: 'A post',
          tags: '["rust"]',
          slug: 'a-post',
          published: false,
          series_id: null,
          series_order: null,
        };
      case 'read_post_markdown':
        return '# A post\n';
      case 'stage_post_thumbnail':
        return null;
      case 'check_post_before_publish':
        return [];
      case 'rename_post_slug':
        return { slug: 'renamed' };
      case 'save_post':
        return { id: 7, slug: 'a-post', title: 'A post', published: true };
      default:
        return [];
    }
  });
};

describe('the thumbnail control', () => {
  beforeEach(() => {
    invoke.mockReset();
    openNewPost();
  });

  /// The thumbnail's key is derived from the slug alone, and a post that has
  /// never been saved has no slug — so there is nowhere for one to go yet.
  it('is disabled, with a reason, on a post that has never been saved', async () => {
    backend();
    render(<PostEditor />);

    const button = await screen.findByRole('button', { name: 'Set thumbnail' });
    expect(button).toBeDisabled();
    expect(button).toHaveAttribute('title', expect.stringContaining('Save the post first'));
  });

  it('offers to set one on a saved post that has none', async () => {
    openPost(7);
    backend();
    render(<PostEditor />);

    const button = await screen.findByRole('button', { name: 'Set thumbnail' });
    await waitFor(() => expect(button).toBeEnabled());
    expect(screen.queryByAltText('Current thumbnail')).not.toBeInTheDocument();
  });

  it('shows the existing thumbnail and offers to replace it', async () => {
    openPost(7);
    backend({ stage_post_thumbnail: 'assets/abc.avif' });
    render(<PostEditor />);

    const image = await screen.findByAltText('Current thumbnail');
    expect(image).toHaveAttribute('src', 'asset://localhost//data/assets/abc.avif');
    expect(await screen.findByRole('button', { name: 'Replace thumbnail' })).toBeInTheDocument();
  });

  /// `AppError::Cancelled` serializes to exactly `"cancelled"`, and a dismissed
  /// dialog must not read as a failure — the same distinction the posts list
  /// draws around `export_post`.
  it('says nothing when the file dialog is dismissed, and stays usable', async () => {
    openPost(7);
    backend({ set_post_thumbnail: rejects('cancelled') });
    render(<PostEditor />);

    const button = await screen.findByRole('button', { name: 'Set thumbnail' });
    await waitFor(() => expect(button).toBeEnabled());
    await userEvent.click(button);

    await waitFor(() => expect(button).toBeEnabled());
    expect(screen.queryByText(/cancelled/)).not.toBeInTheDocument();
  });

  it('reports a real failure', async () => {
    openPost(7);
    backend({ set_post_thumbnail: rejects('R2 rejected the upload') });
    render(<PostEditor />);

    const button = await screen.findByRole('button', { name: 'Set thumbnail' });
    await waitFor(() => expect(button).toBeEnabled());
    await userEvent.click(button);

    expect(await screen.findByText(/R2 rejected the upload/)).toBeInTheDocument();
  });
});

const publish = async () => {
  const button = await screen.findByRole('button', { name: 'Publish' });
  await waitFor(() => expect(button).toBeEnabled());
  await userEvent.click(button);
};

const calls = (command: string) => invoke.mock.calls.filter((c) => c[0] === command);

describe('the pre-publish check', () => {
  beforeEach(() => {
    invoke.mockReset();
    openPost(7);
  });

  it('publishes with no extra step when there is nothing to say', async () => {
    backend();
    render(<PostEditor />);
    await publish();

    await waitFor(() => expect(calls('save_post').length).toBe(1));
    expect(screen.queryByText(/Worth a look/)).not.toBeInTheDocument();
  });

  it('holds the publish and says what it found', async () => {
    backend({ check_post_before_publish: [{ kind: 'no_excerpt' }] });
    render(<PostEditor />);
    await publish();

    expect(await screen.findByText(/Worth a look/)).toBeInTheDocument();
    expect(screen.getByText(/No excerpt/)).toBeInTheDocument();
    // Held, not refused — nothing has gone up.
    expect(calls('save_post')).toHaveLength(0);
  });

  it('names the reference that would publish as a dead link', async () => {
    backend({
      check_post_before_publish: [{ kind: 'dead_asset', reference: 'assets/gone.png' }],
    });
    render(<PostEditor />);
    await publish();

    expect(await screen.findByText('assets/gone.png')).toBeInTheDocument();
  });

  /// The whole point: a warning is not a refusal.
  it('publishes anyway when the author says so, and does not ask twice', async () => {
    backend({ check_post_before_publish: [{ kind: 'no_excerpt' }] });
    render(<PostEditor />);
    await publish();

    await userEvent.click(await screen.findByRole('button', { name: 'Publish anyway' }));

    await waitFor(() => expect(calls('save_post').length).toBe(1));
    // Checked once. Asking again on the way past would be an argument.
    expect(calls('check_post_before_publish')).toHaveLength(1);
  });

  it('lets the author go back without publishing', async () => {
    backend({ check_post_before_publish: [{ kind: 'no_excerpt' }] });
    render(<PostEditor />);
    await publish();

    await userEvent.click(await screen.findByRole('button', { name: 'Go back' }));

    await waitFor(() => expect(screen.queryByText(/Worth a look/)).not.toBeInTheDocument());
    expect(calls('save_post')).toHaveLength(0);
  });

  /// A check that cannot run must not stand between somebody and publishing.
  it('publishes when the check itself fails', async () => {
    backend();
    invoke.mockImplementation(async (command: string) => {
      if (command === 'check_post_before_publish') throw 'no credentials configured';
      if (command === 'get_post') {
        return {
          title: 'A post',
          tags: '["rust"]',
          slug: 'a-post',
          published: false,
          series_id: null,
          series_order: null,
        };
      }
      if (command === 'read_post_markdown') return '# A post\n';
      if (command === 'save_post') return { id: 7, slug: 'a-post', title: 'A post', published: true };
      return [];
    });
    render(<PostEditor />);
    await publish();

    await waitFor(() => expect(calls('save_post').length).toBe(1));
    expect(screen.queryByText(/Worth a look/)).not.toBeInTheDocument();
  });
});

/// The slug tests need a post whose slug is worth correcting; the fixture the
/// others share is `a-post`.
const slugPost = {
  title: 'A post',
  tags: '["rust"]',
  slug: 'typoo',
  published: false,
  series_id: null,
  series_order: null,
};

describe('the slug field', () => {
  beforeEach(() => {
    invoke.mockReset();
    openPost(7);
  });

  it('shows the post’s slug', async () => {
    backend({ get_post: slugPost });
    render(<PostEditor />);
    expect(await screen.findByLabelText('Slug')).toHaveValue('typoo');
  });

  it('renames on blur, once', async () => {
    backend({ get_post: slugPost, rename_post_slug: { slug: 'typo' } });
    render(<PostEditor />);

    const field = await screen.findByLabelText('Slug');
    await waitFor(() => expect(field).toHaveValue('typoo'));
    await userEvent.clear(field);
    await userEvent.type(field, 'typo');
    await userEvent.tab();

    await waitFor(() => expect(calls('rename_post_slug')).toHaveLength(1));
    expect(calls('rename_post_slug')[0][1]).toMatchObject({ id: 7, slug: 'typo' });
    await waitFor(() => expect(field).toHaveValue('typo'));
  });

  /// Typing is not renaming. Every keystroke reaching the backend would rename
  /// the post to each prefix of what was meant.
  it('does not rename while the slug is being typed', async () => {
    backend({ get_post: slugPost, rename_post_slug: { slug: 'typo' } });
    render(<PostEditor />);

    const field = await screen.findByLabelText('Slug');
    await waitFor(() => expect(field).toHaveValue('typoo'));
    await userEvent.clear(field);
    await userEvent.type(field, 'typo');

    expect(calls('rename_post_slug')).toHaveLength(0);
  });

  it('does not call the backend when the slug is unchanged', async () => {
    backend({ get_post: slugPost });
    render(<PostEditor />);

    const field = await screen.findByLabelText('Slug');
    await waitFor(() => expect(field).toHaveValue('typoo'));
    await userEvent.click(field);
    await userEvent.tab();

    expect(calls('rename_post_slug')).toHaveLength(0);
  });

  /// How a published post is protected: the backend refuses and says why. The
  /// box must go back to the slug the post actually has — leaving the refused
  /// text there would show a slug that does not exist next to the reason it
  /// cannot.
  it('puts the real slug back when the rename is refused, and says why', async () => {
    backend({
      get_post: slugPost,
      rename_post_slug: rejects('`typoo` has already been published, so its slug is what readers use.'),
    });
    render(<PostEditor />);

    const field = await screen.findByLabelText('Slug');
    await waitFor(() => expect(field).toHaveValue('typoo'));
    await userEvent.clear(field);
    await userEvent.type(field, 'something-else');
    await userEvent.tab();

    expect(await screen.findByText(/already been published/)).toBeInTheDocument();
    await waitFor(() => expect(field).toHaveValue('typoo'));
  });
});
