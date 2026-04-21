import type { Post } from "../lib/data";

export function StatusDot({ status }: { status: Post["status"] }) {
  return (
    <span
      className={[
        "inline-block w-[6px] h-[6px] rounded-full shrink-0",
        status === "published" ? "bg-emerald-500" : "bg-amber-400",
      ].join(" ")}
    />
  );
}
