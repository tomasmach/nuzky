import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { ZoomIn } from "lucide-react";
import { api, errorText } from "../../lib/api";
import { aiLocked, applyZooms, useEditor } from "../../lib/store";
import type { SuggestedZoom, TranscriptView, ZoomSuggestions } from "../../lib/types";
import { Button } from "../ui";

/** "1.24×" */
export const zoomLabel = (scale: number) => `${scale.toFixed(2).replace(/0$/, "")}×`;

export interface ZoomSuggestionState {
  /** Suggested sentences of the current words, or null when none are shown. */
  zooms: SuggestedZoom[] | null;
  pending: boolean;
  suggest: () => Promise<void>;
  apply: () => Promise<void>;
  dismiss: () => void;
}

/**
 * Suggested punch-ins for the transcript. They belong to the words they were made for, so they go
 * when the speech changes or is recognised again.
 */
export function useZoomSuggestions(view: TranscriptView | null): ZoomSuggestionState {
  const [found, setFound] = useState<ZoomSuggestions | null>(null);
  const [pending, setPending] = useState(false);
  const key = view?.key;
  useEffect(() => setFound((f) => (f && f.key === key ? f : null)), [key]);

  const suggest = async () => {
    setPending(true);
    try {
      const next = await api.suggestZooms();
      if (next.zooms.length === 0) {
        useEditor.getState().toast({ kind: "info", text: "No sentence stands out enough for a zoom." });
        setFound(null);
      } else setFound(next);
    } catch (e) {
      useEditor.getState().toast({ kind: "error", text: errorText(e) });
    } finally {
      setPending(false);
    }
  };

  const apply = async () => {
    if (!found || aiLocked()) return;
    setPending(true);
    try {
      if (await applyZooms(found.key, found.zooms.map(({ from, to, scale }) => ({ from, to, scale })))) setFound(null);
    } finally {
      setPending(false);
    }
  };

  return { zooms: found && found.key === key ? found.zooms : null, pending, suggest, apply, dismiss: () => setFound(null) };
}

/** Opens the suggestions; disabled with the reason while the words are incomplete or recognition runs. */
export function SuggestZoomsButton({ state, blocker }: { state: ZoomSuggestionState; blocker: string | null }) {
  return (
    <Button
      className="h-7 px-2"
      data-suggest-zooms
      disabled={!!blocker || state.pending}
      disabledReason={blocker ?? "Finding sentences to zoom on…"}
      title="Mark the sentences said with emphasis for a subtle zoom"
      onClick={state.suggest}
    >
      <ZoomIn size={14} /> Suggest zooms
    </Button>
  );
}

/**
 * The suggestions' bar above the text: how many, Dismiss and Apply. Apply takes focus when the
 * suggestions arrive, Esc dismisses them, and focus then returns to Suggest zooms.
 */
export function ZoomBar({ state, blocker }: { state: ZoomSuggestionState; blocker: string | null }) {
  const bar = useRef<HTMLDivElement>(null);
  const zooms = state.zooms;
  const shown = !!zooms;
  useEffect(() => {
    if (shown) bar.current?.querySelector<HTMLElement>("[data-zoom-apply]")?.focus();
  }, [shown]);
  if (!zooms) return null;
  const back = () => document.querySelector<HTMLElement>("[data-suggest-zooms]")?.focus();
  const close = (run: () => void | Promise<void>) => async () => {
    const inside = !!document.activeElement?.closest("[data-zoom-bar]");
    await run();
    if (inside) back();
  };
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key !== "Escape") return;
    e.stopPropagation();
    state.dismiss();
    back();
  };
  const n = zooms.length;
  const scales = [...new Set(zooms.map((z) => z.scale))].sort((a, b) => a - b);
  const range = scales.length === 1 ? zoomLabel(scales[0]) : `${zoomLabel(scales[0])}–${zoomLabel(scales[scales.length - 1])}`;
  return (
    <div ref={bar} role="region" aria-label="Suggested zooms" data-zoom-bar className="flex shrink-0 items-center gap-2 border-b border-line px-3 py-2" onKeyDown={onKeyDown}>
      <ZoomIn size={14} className="shrink-0 text-accent" aria-hidden />
      <span className="tabular flex-1 text-[12px] text-muted">
        {n} sentence{n === 1 ? "" : "s"} · {range}
      </span>
      <Button className="h-7 px-2" onClick={close(state.dismiss)}>
        Dismiss
      </Button>
      <Button data-zoom-apply variant="primary" className="h-7 px-2" disabled={!!blocker || state.pending} disabledReason={blocker ?? "Applying…"} onClick={close(state.apply)}>
        Apply {n} zoom{n === 1 ? "" : "s"}
      </Button>
    </div>
  );
}
