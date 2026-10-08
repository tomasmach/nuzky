import type { LucideIcon } from "lucide-react";

/** The quiet 24 px icon buttons of a section header: keyframes and reset. */
export const QUIET = "size-6 rounded-md";

/** What the inspector shows: kind icon and name, with a tabular detail line below. */
export function InspectorHeader({ icon: Icon, title, detail }: { icon: LucideIcon; title: string; detail: string }) {
  return (
    <div className="shrink-0 px-4 pb-3 pt-4">
      <div className="flex items-center gap-2 text-[15px] font-semibold leading-5 tracking-[-0.015em] text-fg">
        <Icon size={16} className="shrink-0 text-muted" />
        <span className="truncate">{title}</span>
      </div>
      <div className="tabular mt-[3px] text-[12px] text-muted">{detail}</div>
    </div>
  );
}
