/// Tests for the editor's thumbnail control.
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
