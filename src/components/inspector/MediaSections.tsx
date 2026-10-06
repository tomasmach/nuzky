import { RotateCcw, Unlink } from "lucide-react";
import { MAX_SPEED, MIN_SPEED, NO_ADJUST, SPEED_PRESETS, sameAdjust } from "../../lib/presets";
import { detachAudio, detachBlocker, editClip, findClip, useEditor } from "../../lib/store";
import { US, formatDuration } from "../../lib/time";
import type { Adjust, Asset, Clip } from "../../lib/types";
import { Button, IconButton, Section, Segmented, Slider } from "../ui";

type Media = Extract<Clip["content"], { type: "media" }>;

const ADJUST_ROWS: { key: keyof Adjust; label: string; min: number }[] = [
  { key: "brightness", label: "Brightness", min: -100 },
  { key: "contrast", label: "Contrast", min: -100 },
  { key: "saturation", label: "Saturation", min: -100 },
  { key: "temperature", label: "Temperature", min: -100 },
  { key: "vignette", label: "Vignette", min: 0 },
];

export function AdjustSection({ clip, content }: { clip: Clip; content: Media }) {
  const edit = useEditor((s) => s.edit);
  const adjust = content.adjust;
  // Built from the latest confirmed values, so a second slider never overwrites the first.
  const set = (key: keyof Adjust, v: number) =>
    editClip(clip.id, (c) => (c.content.type === "media" ? { type: "updateClip", clipId: c.id, adjust: { ...c.content.adjust, [key]: v / 100 } } : null), `${clip.id}:adjust:${key}`);
  return (
    <Section
      title="Adjust"
      actions={
        <IconButton label="Reset adjustments" className="h-7 w-7" disabled={sameAdjust(adjust, NO_ADJUST)} onClick={() => edit({ type: "updateClip", clipId: clip.id, adjust: NO_ADJUST })}>
          <RotateCcw size={14} />
        </IconButton>
      }
    >
      {ADJUST_ROWS.map((r) => (
        <Slider key={r.key} label={r.label} value={adjust[r.key] * 100} min={r.min} max={100} step={1} format={(v) => String(Math.round(v))} onChange={(v) => set(r.key, v)} />
      ))}
    </Section>
  );
}

export function SpeedSection({ clip, content, asset }: { clip: Clip; content: Media; asset?: Asset }) {
  const edit = useEditor((s) => s.edit);
  const speed = content.speed;
  const set = (v: number, key = "speed") => edit({ type: "updateClip", clipId: clip.id, speed: Math.round(v * 100) / 100 }, `${clip.id}:${key}`);
  const sourceUs = clip.durationUs * speed;
  return (
    <Section title="Speed">
      <Segmented
        label="Speed presets"
        value={SPEED_PRESETS.find((p) => Math.abs(p - speed) < 0.005) ?? null}
        onChange={(p) => set(p, `speed-preset:${Date.now()}`)}
        options={SPEED_PRESETS.map((p) => ({ id: p, label: `${p}x` }))}
      />
      {/* Logarithmic so 0.1x–1x gets as much travel as 1x–10x. */}
      <Slider
        label="Speed"
        value={Math.log10(speed)}
        min={Math.log10(MIN_SPEED)}
        max={Math.log10(MAX_SPEED)}
        step={0.01}
        unit="x"
        format={(v) => (10 ** v).toFixed(2)}
        parse={(s) => Math.log10(Number(s.replace(",", ".").replace(/[^\d.]/g, "")))}
        onChange={(v) => set(10 ** v)}
      />
      <dl className="tabular grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-[12px]">
        <dt className="text-muted">Duration</dt>
        <dd className="text-fg">{formatDuration(clip.durationUs)}</dd>
        <dt className="text-muted">Source used</dt>
        <dd className="text-fg">{formatDuration(sourceUs)}</dd>
      </dl>
      {asset?.hasAudio && speed !== 1 && <p className="text-[12px] text-muted">The pitch of the sound changes with the speed.</p>}
    </Section>
  );
}

export function AudioSection({ clip, content }: { clip: Clip; content: Media }) {
  const project = useEditor((s) => s.snap!.project);
  const edit = useEditor((s) => s.edit);
  const maxFade = Math.min(10, Math.floor((clip.durationUs / 2 / US) * 10) / 10);
  const blocker = detachBlocker(project, clip);
  // Sound already detached to an audio track has nothing left to detach.
  const isVideo = project.assets.find((a) => a.id === content.assetId)?.kind === "video" && findClip(project, clip.id)?.track.kind !== "audio";
  return (
    <Section title="Audio">
      <Slider
        label="Volume"
        value={content.volume * 100}
        min={0}
        max={200}
        step={1}
        unit="%"
        format={(v) => String(Math.round(v))}
        onChange={(v) => edit({ type: "updateClip", clipId: clip.id, volume: v / 100 }, `${clip.id}:volume`)}
      />
      <Slider
        label="Fade in"
        value={content.fadeInUs / US}
        min={0}
        max={maxFade}
        step={0.1}
        unit="s"
        format={(v) => v.toFixed(1)}
        onChange={(v) => edit({ type: "updateClip", clipId: clip.id, fadeInUs: Math.round(v * US) }, `${clip.id}:fadeIn`)}
      />
      <Slider
        label="Fade out"
        value={content.fadeOutUs / US}
        min={0}
        max={maxFade}
        step={0.1}
        unit="s"
        format={(v) => v.toFixed(1)}
        onChange={(v) => edit({ type: "updateClip", clipId: clip.id, fadeOutUs: Math.round(v * US) }, `${clip.id}:fadeOut`)}
      />
      {isVideo && (
        <span className="block" title={blocker ?? "Move the sound to its own audio track"}>
          <Button className="w-full" disabled={!!blocker} onClick={() => detachAudio(clip.id)}>
            <Unlink size={14} /> Detach audio
          </Button>
        </span>
      )}
    </Section>
  );
}
