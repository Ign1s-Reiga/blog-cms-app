import { PostEditor } from "../../components/PostEditor";

// Editor for an existing post. The post to load comes from the `?id=` query
// param, which PostEditor reads on mount.
export default function EditPostPage() {
  return (
    <main className="flex-1 overflow-hidden flex flex-col">
      <PostEditor />
    </main>
  );
}
