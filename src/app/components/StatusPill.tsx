import type { Post } from "../lib/data";
import { StatusDot } from "./StatusDot";

export function StatusPill({ status }: { status: Post["status"] }) {
  return status === "published" ? (
    <span className="inline-flex items-center gap-[5px] px-[7px] py-[3px] rounded-full text-[11px] font-semibold bg-emerald-50 text-emerald-700 dark:bg-emerald-500/[0.12] dark:text-emerald-400 border border-emerald-200/80 dark:border-emerald-500/20">
      <StatusDot status="published" />
      Published
    </span>
  ) : (
    <span className="inline-flex items-center gap-[5px] px-[7px] py-[3px] rounded-full text-[11px] font-semibold bg-amber-50 text-amber-700 dark:bg-amber-500/[0.12] dark:text-amber-400 border border-amber-200/80 dark:border-amber-500/20">
      <StatusDot status="draft" />
      Draft
    </span>
  );
}
