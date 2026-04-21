import { PlaceholderView } from "../components/PlaceholderView";

export default function MediaPage() {
  return (
    <main className="flex-1 overflow-y-auto p-6">
      <PlaceholderView
        icon="folder"
        title="Media Library"
        desc="Browse and manage images and videos stored in Cloudflare R2."
      />
    </main>
  );
}
