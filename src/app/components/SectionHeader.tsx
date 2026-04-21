export function SectionHeader({
  children,
  action,
}: {
  children: React.ReactNode;
  action?: React.ReactNode;
}) {
  return (
    <div className="flex items-center gap-3 mb-5">
      <h2 className="text-[10px] font-bold text-zinc-400 dark:text-zinc-600 uppercase tracking-[0.12em] shrink-0">
        {children}
      </h2>
      <div className="flex-1 h-px bg-zinc-100 dark:bg-white/[0.04]" />
      {action}
    </div>
  );
}
