import { useState } from "react";
import { Ban } from "lucide-react";
import { ANIMATIONS } from "../../lib/presets";
import { useEditor } from "../../lib/store";
import { US } from "../../lib/time";
import type { AnimationKind, Clip } from "../../lib/types";
import { PresetTile, Section, Segmented, Slider } from "../ui";

const DEFAULT_US = 500_000;

function AnimationPreview({ kind, out, thumb, text }: { kind: AnimationKind; out: boolean; thumb: string | null; text: boolean }) {
  return (
    <span className="absolute inset-0 flex items-center justify-center overflow-hidden">
      <span className={`pv ${out ? "pv-out" : ""} ${kind === "typewriter" ? "a-typewriter" : ""}`} style={{ animationName: `a-${kind}` }}>
        {text ? (
          <span className="block text-[16px] font-extrabold leading-none text-fg">Text</span>
        ) : (
          <span className="block h-8 w-12 rounded-sm bg-clip-video bg-cover bg-center" style={thumb ? { backgroundImage: `url(${thumb})` } : undefined} />
        )}
      </span>
    </span>
  );
}

export function AnimationSection({ clip }: { clip: Clip }) {
  const [slot, setSlot] = useState<"in" | "out">("in");
  const edit = useEditor((s) => s.edit);
  const thumb = useEditor((s) => (clip.content.type === "media" ? (s.thumbs[clip.content.assetId] ?? null) : null));
  const text = clip.content.type === "text";
  const current = slot === "in" ? clip.animIn : clip.animOut;
  const maxS = Math.max(0.1, Math.min(5, Math.floor((clip.durationUs / US) * 10) / 10));
  const durationUs = Math.min(current?.durationUs ?? DEFAULT_US, maxS * US);

  const pick = (kind: AnimationKind | null) => edit({ type: "setAnimation", clipId: clip.id, slot, animation: kind ? { kind, durationUs } : null });
  const dot = (set: boolean) => (set ? <span aria-label="set" className="ml-1 inline-block h-1.5 w-1.5 rounded-full bg-accent align-middle" /> : null);

  return (
    <Section title="Animation">
      <Segmented
        label="Animation slot"
        value={slot}
        onChange={setSlot}
        options={[
          { id: "in", label: <>In{dot(!!clip.animIn)}</>, title: clip.animIn ? `In: ${ANIMATIONS.find((a) => a.kind === clip.animIn!.kind)?.label}` : "Entry animation" },
          { id: "out", label: <>Out{dot(!!clip.animOut)}</>, title: clip.animOut ? `Out: ${ANIMATIONS.find((a) => a.kind === clip.animOut!.kind)?.label}` : "Exit animation" },
        ]}
      />
      <div className="grid grid-cols-3 gap-2">
        <PresetTile label="None" selected={!current} onClick={() => pick(null)} title="No animation">
          <span className="absolute inset-0 flex items-center justify-center text-muted">
            <Ban size={18} />
          </span>
        </PresetTile>
        {ANIMATIONS.filter((a) => text || !a.textOnly).map((a) => (
          <PresetTile key={a.kind} label={a.label} selected={current?.kind === a.kind} onClick={() => pick(a.kind)}>
            <AnimationPreview kind={a.kind} out={slot === "out"} thumb={thumb} text={text} />
          </PresetTile>
        ))}
      </div>
      <Slider
        label="Duration"
        value={durationUs / US}
        min={0.1}
        max={maxS}
        step={0.1}
        unit="s"
        format={(v) => v.toFixed(1)}
        disabled={!current}
        title={current ? undefined : "Pick an animation first"}
        onChange={(v) => current && edit({ type: "setAnimation", clipId: clip.id, slot, animation: { ...current, durationUs: Math.round(v * US) } }, `${clip.id}:anim-${slot}`)}
      />
    </Section>
  );
}
