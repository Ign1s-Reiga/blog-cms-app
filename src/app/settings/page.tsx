import { PlaceholderView } from "@/components/PlaceholderView";

export default function SettingsPage() {
  return (
    <main className="flex-1 overflow-y-auto p-6">
      <PlaceholderView
        icon="settings"
        title="Settings"
        desc="Configure Cloudflare credentials, sync intervals, and preferences."
      />
    </main>
  );
}
