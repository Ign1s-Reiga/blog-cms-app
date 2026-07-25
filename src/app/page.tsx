import { ArrowUpRight, Plus, RefreshCw, TrendingUp, Upload } from "lucide-react";
import Link from "next/link";
import { POSTS, STATS } from "@/lib/data";
import { SectionHeader } from "@/components/SectionHeader";
import { StatusDot } from "@/components/StatusDot";
import { Card } from "@/components/ui/card";

// ─── Quick actions data ───────────────────────────────────────────────────────

const QUICK_ACTIONS = [
  {
    label: "New Post",
    desc:  "Start writing in Markdown",
    Icon:  Plus,
    href:  "/posts/new",
  },
  {
    label: "Upload Media",
    desc:  "Add files to Cloudflare R2",
    Icon:  Upload,
    href:  "/media",
  },
  {
    label: "Sync Cloud",
    desc:  "Push local drafts to cloud",
    Icon:  RefreshCw,
    href:  null,
  },
] as const;

// ─── Page ─────────────────────────────────────────────────────────────────────

export default function DashboardPage() {
  return (
    <main className="flex-1 overflow-y-auto p-6">
      <div className="space-y-10 w-full">

        {/* Stats */}
        <section>
          <SectionHeader>Overview</SectionHeader>
          <div className="grid grid-cols-2 lg:grid-cols-4 gap-3">
            {STATS.map(({ label, value, Icon, delta, positive }) => (
              <Card
                key={label}
                className={[
                  "group relative flex flex-col justify-between",
                  "p-4 rounded-lg gap-0 ring-0",
                  "bg-white dark:bg-[#161616]",
                  "border border-zinc-200 dark:border-white/[0.07]",
                  "hover:border-zinc-300 dark:hover:border-white/12",
                  "hover:shadow-[0_4px_20px_rgba(0,0,0,0.06)] dark:hover:shadow-[0_4px_20px_rgba(0,0,0,0.4)]",
                  "transition-[border-color,box-shadow] duration-200",
                ].join(" ")}
              >
                <div className="flex items-center justify-between gap-2">
                  <span className="text-[10px] font-bold text-zinc-400 dark:text-zinc-600 uppercase tracking-widest">
                    {label}
                  </span>
                  <div className="p-1.25 rounded-[5px] bg-zinc-50 dark:bg-white/4 border border-zinc-100 dark:border-white/6">
                    <Icon size={12} strokeWidth={1.8} className="text-zinc-400 dark:text-zinc-600" />
                  </div>
                </div>

                <p className="mt-3 text-[28px] font-bold tracking-tight leading-none text-zinc-900 dark:text-zinc-50 tabular-nums">
                  {value}
                </p>

                <div className="mt-3 pt-3 border-t border-zinc-100 dark:border-white/4 flex items-center gap-1">
                  {positive && (
                    <TrendingUp size={11} strokeWidth={2} className="text-emerald-500 shrink-0" />
                  )}
                  <span
                    className={[
                      "text-[11px] font-medium leading-none",
                      positive
                        ? "text-emerald-600 dark:text-emerald-400"
                        : "text-zinc-400 dark:text-zinc-600",
                    ].join(" ")}
                  >
                    {delta}
                  </span>
                </div>
              </Card>
            ))}
          </div>
        </section>

        {/* Recent posts */}
        <section>
          <SectionHeader
            action={
              <Link
                href="/posts"
                className="flex items-center gap-0.75 text-[11px] font-semibold text-zinc-400 dark:text-zinc-600 hover:text-zinc-700 dark:hover:text-zinc-300 transition-colors duration-100 active:scale-95 shrink-0"
              >
                View all <ArrowUpRight size={11} strokeWidth={2} />
              </Link>
            }
          >
            Recent Posts
          </SectionHeader>

          <div className="rounded-lg border border-zinc-200 dark:border-white/[0.07] overflow-hidden bg-white dark:bg-[#161616]">
            {POSTS.slice(0, 4).map((post, i) => (
              <div
                key={post.id}
                className={[
                  "group flex items-center gap-3 px-4 py-2.75",
                  "hover:bg-zinc-50 dark:hover:bg-white/3",
                  "transition-colors duration-100",
                  i < 3 ? "border-b border-zinc-100 dark:border-white/4" : "",
                ].join(" ")}
              >
                <StatusDot status={post.status} />

                <div className="flex-1 min-w-0">
                  <p className="text-[13px] font-medium text-zinc-800 dark:text-zinc-200 truncate leading-none group-hover:text-zinc-900 dark:group-hover:text-white transition-colors duration-100">
                    {post.title}
                  </p>
                  <div className="flex items-center gap-2 mt-1.25">
                    <span className="text-[11px] font-mono text-zinc-400 dark:text-zinc-600 tracking-tight">
                      {post.date}
                    </span>
                    {post.tags.map((t) => (
                      <span key={t} className="text-[11px] text-zinc-400 dark:text-zinc-600">
                        · {t}
                      </span>
                    ))}
                  </div>
                </div>

                {post.views !== undefined && (
                  <span className="text-[12px] font-mono tabular-nums text-zinc-400 dark:text-zinc-600 hidden sm:block shrink-0">
                    {post.views.toLocaleString()}
                  </span>
                )}

                <ArrowUpRight
                  size={13}
                  strokeWidth={1.8}
                  className="shrink-0 text-zinc-300 dark:text-zinc-700 group-hover:text-zinc-500 dark:group-hover:text-zinc-400 transition-colors duration-100"
                />
              </div>
            ))}
          </div>
        </section>

        {/* Quick actions */}
        <section>
          <SectionHeader>Quick Actions</SectionHeader>
          <div className="grid grid-cols-1 sm:grid-cols-3 gap-3">
            {QUICK_ACTIONS.map(({ label, desc, Icon, href }) => {
              const inner = (
                <>
                  <div className="p-1.75 rounded-md bg-zinc-50 dark:bg-white/[0.05] border border-zinc-100 dark:border-white/[0.06] group-hover:bg-zinc-100 dark:group-hover:bg-white/[0.08] transition-colors duration-150 shrink-0 mt-px">
                    <Icon size={13} strokeWidth={2} className="text-zinc-500 dark:text-zinc-400" />
                  </div>
                  <div className="flex-1 min-w-0">
                    <p className="text-[13px] font-semibold text-zinc-800 dark:text-zinc-200 leading-none">
                      {label}
                    </p>
                    <p className="text-[11px] text-zinc-400 dark:text-zinc-600 mt-1.25 leading-tight">
                      {desc}
                    </p>
                  </div>
                  <ArrowUpRight
                    size={13}
                    strokeWidth={1.8}
                    className="shrink-0 text-zinc-300 dark:text-zinc-700 group-hover:text-zinc-500 dark:group-hover:text-zinc-400 mt-px transition-colors duration-150"
                  />
                </>
              );

              const cardClass = [
                "group flex items-start gap-3 p-4 text-left rounded-[8px]",
                "bg-white dark:bg-[#161616]",
                "border border-zinc-200 dark:border-white/[0.07]",
                "hover:border-zinc-300 dark:hover:border-white/[0.12]",
                "hover:shadow-[0_4px_16px_rgba(0,0,0,0.05)] dark:hover:shadow-[0_4px_16px_rgba(0,0,0,0.4)]",
                "active:scale-[0.975] active:translate-y-px active:shadow-none active:border-zinc-200 dark:active:border-white/[0.07] active:transition-none",
                "transition-[border-color,box-shadow,transform] duration-150",
              ].join(" ");

              return href ? (
                <Link key={label} href={href} className={cardClass}>
                  {inner}
                </Link>
              ) : (
                <div key={label} className={[cardClass, "opacity-50 cursor-not-allowed"].join(" ")}>
                  {inner}
                </div>
              );
            })}
          </div>
        </section>
      </div>
    </main>
  );
}
