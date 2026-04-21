"use client";

import { useEffect, useState } from "react";
import { useTheme } from "next-themes";
import { Moon, Sun } from "lucide-react";

export function ThemeToggle() {
  const { theme, setTheme } = useTheme();
  const [mounted, setMounted] = useState(false);

  // Avoid hydration mismatch — render only after client mount.
  useEffect(() => setMounted(true), []);

  return (
    <button
      onClick={() => setTheme(theme === "dark" ? "light" : "dark")}
      aria-label="Toggle theme"
      className={[
        "w-[30px] h-[30px] flex items-center justify-center rounded-[6px]",
        "text-zinc-400 dark:text-zinc-500",
        "hover:bg-zinc-100 dark:hover:bg-white/[0.06]",
        "hover:text-zinc-700 dark:hover:text-zinc-300",
        "active:scale-[0.92] active:bg-zinc-200 dark:active:bg-white/[0.1] active:transition-none",
        "transition-[background-color,color,transform] duration-100",
      ].join(" ")}
    >
      {mounted && theme === "dark"
        ? <Sun  size={15} strokeWidth={1.8} />
        : <Moon size={15} strokeWidth={1.8} />
      }
    </button>
  );
}
