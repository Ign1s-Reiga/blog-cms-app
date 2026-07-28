import { StatusDot, type PostStatus } from "./StatusDot";
import { Badge } from "@/components/ui/badge";

export function StatusPill({ status }: { status: PostStatus }) {
  if (status === "failed") {
    return (
      <Badge
        variant="outline"
        className="gap-[5px] px-[7px] py-[3px] rounded-full text-[11px] font-semibold bg-red-50 text-red-700 dark:bg-red-500/[0.12] dark:text-red-400 border-red-200/80 dark:border-red-500/20"
      >
        <StatusDot status="failed" />
        Sync failed
      </Badge>
    );
  }

  return status === "published" ? (
    <Badge
      variant="outline"
      className="gap-[5px] px-[7px] py-[3px] rounded-full text-[11px] font-semibold bg-emerald-50 text-emerald-700 dark:bg-emerald-500/[0.12] dark:text-emerald-400 border-emerald-200/80 dark:border-emerald-500/20"
    >
      <StatusDot status="published" />
      Published
    </Badge>
  ) : (
    <Badge
      variant="outline"
      className="gap-[5px] px-[7px] py-[3px] rounded-full text-[11px] font-semibold bg-amber-50 text-amber-700 dark:bg-amber-500/[0.12] dark:text-amber-400 border-amber-200/80 dark:border-amber-500/20"
    >
      <StatusDot status="draft" />
      Draft
    </Badge>
  );
}
