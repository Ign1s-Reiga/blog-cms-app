"use client";

import { ChevronRight } from "lucide-react";
import { usePathname } from "next/navigation";

const LABELS: Record<string, string> = {
  "/":           "Dashboard",
  "/posts":      "Posts",
  "/posts/new":  "New Post",
  "/media":      "Media",
  "/analytics":  "Analytics",
  "/settings":   "Settings",
};

export function Breadcrumb() {
  const pathname = usePathname();
  const label = LABELS[pathname] ?? "Page";

  return (
    <nav aria-label="Breadcrumb" className="flex items-center gap-[5px] text-[13px]">
      <span className="text-zinc-400 dark:text-zinc-600 font-medium hidden sm:block">
        blog-cms
      </span>
      <ChevronRight
        size={13}
        strokeWidth={1.8}
        className="text-zinc-300 dark:text-zinc-700 hidden sm:block"
      />
      <span className="font-semibold text-zinc-800 dark:text-zinc-200">
        {label}
      </span>
    </nav>
  );
}
