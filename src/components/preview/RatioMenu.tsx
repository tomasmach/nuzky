import { useEffect, useRef, type KeyboardEvent } from "react";
import { Check, ChevronDown, RectangleHorizontal } from "lucide-react";
import { FORMATS, formatLabel } from "../../lib/presets";
import { picturedClips, reframe, useReframe } from "../../lib/reframe";
import { AI_EDITING, useAiLocked, useEditor } from "../../lib/store";
import { Button } from "../ui";

/**
 * CapCut's "Ratio" control next to the player: the one place to change the canvas format, and beside each format
 * Reframe, which also places the clips to follow the face. Open state lives in the store so the Format row in the
 * Project inspector can open it too.
 */
export function RatioMenu() {
  const canvas = useEditor((s) => s.snap!.project.canvas);
  const pictures = useEditor((s) => s.snap!.project.tracks.some((t) => t.kind === "video" && t.clips.some((c) => c.content.type === "media")));
  const selected = useEditor((s) => picturedClips(s.snap!.project, s.selection).length);
  const reframing = useReframe((s) => !!s.job);
  const edit = useEditor((s) => s.edit);
  const open = useEditor((s) => s.ratioOpen);
  const locked = useAiLocked();
  const setOpen = (ratioOpen: boolean) => useEditor.setState({ ratioOpen });
  const ref = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const closeToTrigger = () => {
    setOpen(false);
    trigger.current?.focus();
  };

  useEffect(() => {
    if (!open) return;
    ref.current?.querySelector<HTMLElement>("[aria-checked=true]")?.focus();
    const close = (e: PointerEvent) => !ref.current?.contains(e.target as Node) && setOpen(false);
    const key = (e: globalThis.KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        closeToTrigger();
      }
    };
    window.addEventListener("pointerdown", close);
    window.addEventListener("keydown", key, true);
    return () => {
      window.removeEventListener("pointerdown", close);
      window.removeEventListener("keydown", key, true);
    };
  }, [open]);

  // Like the clip menu: ↑/↓ move, through each format and its Reframe, and no key reaches the editor's shortcuts.
  const onMenuKey = (e: KeyboardEvent<HTMLDivElement>) => {
    e.stopPropagation();
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    e.preventDefault();
    const items = [...e.currentTarget.querySelectorAll<HTMLElement>("[role=menuitemradio], [role=menuitem]")];
    const i = items.indexOf(document.activeElement as HTMLElement);
    items[(i + (e.key === "ArrowDown" ? 1 : items.length - 1)) % items.length]?.focus();
  };

  return (
    <div className="relative" ref={ref}>
      <button
        ref={trigger}
        type="button"
        data-ratio
        aria-haspopup="menu"
        aria-expanded={open}
        title={locked ? AI_EDITING : "Canvas ratio"}
        disabled={locked}
        onClick={() => setOpen(!open)}
        className={`flex h-8 items-center gap-1.5 rounded-full px-3 text-[12px] font-medium transition-colors duration-[120ms] ease-out disabled:cursor-not-allowed disabled:opacity-40 ${
          open ? "bg-white/[.14] text-fg" : "bg-white/[.06] text-fg enabled:hover:bg-white/[.12]"
        }`}
      >
        <RectangleHorizontal size={14} className="text-muted @max-[460px]:hidden" />
        <span className="@max-[460px]:hidden">Ratio</span>
        <span className="tabular font-normal text-muted">{formatLabel(canvas.width, canvas.height)}</span>
        <ChevronDown size={13} className="text-muted" />
      </button>
      {open && (
        <div role="menu" aria-label="Canvas ratio" onKeyDown={onMenuKey} className="overlay absolute bottom-11 right-0 z-50 w-[19rem] rounded-xl p-1">
          {FORMATS.map((f) => {
            const on = f.width === canvas.width && f.height === canvas.height;
            const why = !pictures ? "Add a video to the timeline first" : reframing ? "Already following the face" : null;
            const which = selected > 0 ? `the ${selected} selected clip${selected === 1 ? "" : "s"}` : "every full-frame clip";
            return (
              <div key={f.label} className="group flex items-center gap-1 rounded-md pr-1 hover:bg-white/[.08] has-[:focus-visible]:bg-white/[.08]">
                <button
                  type="button"
                  role="menuitemradio"
                  aria-checked={on}
                  onClick={() => {
                    if (!on) edit({ type: "setCanvas", width: f.width, height: f.height });
                    closeToTrigger();
                  }}
                  className="flex h-8 min-w-0 flex-1 items-center gap-2 rounded-md px-2 text-left focus-visible:outline-offset-[-2px]"
                >
                  <span className="tabular w-9 shrink-0 text-[13px] font-medium text-fg">{f.label}</span>
                  <span className="flex-1 truncate text-[12px] text-muted">{f.hint}</span>
                  {on && <Check size={14} className="shrink-0 text-accent" />}
                </button>
                <span className="shrink-0 opacity-0 group-hover:opacity-100 group-has-[:focus-visible]:opacity-100">
                  <Button
                    pill
                    role="menuitem"
                    className="h-6 px-2.5 text-[12px]"
                    title={on ? `Keep the speaker's face in ${which}` : `Switch to ${f.label} and keep the speaker's face in ${which}`}
                    disabled={!!why}
                    disabledReason={why ?? undefined}
                    onClick={() => {
                      closeToTrigger();
                      void reframe(f.width, f.height);
                    }}
                  >
                    Reframe
                  </Button>
                </span>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
