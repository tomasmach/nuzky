import { RotateCcw, Unlink } from "lucide-react";
import { LIMITS } from "../../lib/limits";
import { ADJUST_ROWS, NO_ADJUST, SPEED_PRESETS, sameAdjust } from "../../lib/presets";
import { detachAudio, detachBlocker, editClip, editClips, findClip, maxFadeUs, setCleanVoice, useEditor, useVoicePreparation } from "../../lib/store";
import { US, formatDuration } from "../../lib/time";
import type { Adjust, Asset, Clip } from "../../lib/types";
import { Button, Checkbox, IconButton, ProgressBar, Section, Segmented, Slider } from "../ui";
import { QUIET } from "./Header";

type Media = Extract<Clip["content"], { type: "media" }>;

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
        <IconButton label="Reset adjustments" className={QUIET} disabled={sameAdjust(adjust, NO_ADJUST)} onClick={() => edit({ type: "updateClip", clipId: clip.id, adjust: NO_ADJUST })}>
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
        min={Math.log10(LIMITS.minSpeed)}
        max={Math.log10(LIMITS.maxSpeed)}
        step={0.01}
        unit="x"
        format={(v) => (10 ** v).toFixed(2)}
        parse={(s) => Math.log10(Number(s.replace(",", ".").replace(/[^\d.]/g, "")))}
        onChange={(v) => set(10 ** v)}
      />
      <dl className="tabular grid grid-cols-[78px_1fr] gap-x-2 gap-y-1.5 text-[12px]">
        <dt className="text-muted">Duration</dt>
        <dd className="text-fg">{formatDuration(clip.durationUs)}</dd>
        <dt className="text-muted">Source used</dt>
        <dd className="text-fg">{formatDuration(sourceUs)}</dd>
      </dl>
      {asset?.hasAudio && speed !== 1 && (
        <Checkbox
          label="Keep pitch"
          checked={content.keepPitch ?? false}
          title="The voice sounds as high as at 1x. Off, it gets higher when faster and lower when slower."
          onChange={(v) => edit({ type: "updateClip", clipId: clip.id, keepPitch: v })}
        />
      )}
    </Section>
  );
}

/**
 * Clean voice for one or more clips with sound. While the cleaned sound of their files is being
 * prepared, playback keeps the original sound and the row shows how far it is.
 */
export function CleanVoiceRow({ clips }: { clips: Clip[] }) {
  const on = clips.map((c) => c.content.type === "media" && c.content.cleanVoice);
  const all = on.every(Boolean);
  const assets = clips.flatMap((c) => (c.content.type === "media" && c.content.cleanVoice ? [c.content.assetId] : []));
  const preparing = useVoicePreparation(assets);
  return (
    <>
      <Checkbox
        label="Clean voice"
        checked={all}
        mixed={!all && on.some(Boolean)}
        title="Less background noise, rumble and harsh s sounds in speech"
        onChange={(v) => setCleanVoice(clips.map((c) => c.id), v)}
      />
      {preparing !== null && (
        <div className="flex flex-col gap-1.5" role="status">
          <span className="tabular text-[12px] text-muted">
            Cleaning voice{preparing > 0 && ` · ${Math.round(preparing * 100)}%`}
          </span>
          <ProgressBar value={preparing} label="Cleaning voice" />
        </div>
      )}
    </>
  );
}

/** How far a clip goes down under speech when Lower under speech is turned on, in dB. */
const DUCK_DB = 12;

/**
 * Lower under speech (ducking) for one or more clips with sound: they go down while a video's own
 * sound has speech and come back in the pauses. The strength shows once all of them have it on.
 */
export function DuckingRow({ clips, coalesce }: { clips: Clip[]; coalesce: string }) {
  const levels = clips.map((c) => (c.content.type === "media" ? (c.content.duckDb ?? 0) : 0));
  const on = levels.filter((v) => v > 0);
  const all = on.length === levels.length;
  const ids = clips.map((c) => c.id);
  // Checking keeps the strength of clips that already have it.
  const turn = (v: boolean) =>
    editClips(ids, (c) => (c.content.type !== "media" || (v && (c.content.duckDb ?? 0) > 0) ? null : { type: "updateClip", clipId: c.id, duckDb: v ? DUCK_DB : 0 }));
  return (
    <>
      <Checkbox
        label="Lower under speech"
        checked={all}
        mixed={!all && on.length > 0}
        title="Turns this sound down while someone speaks in a video and back up in the pauses"
        onChange={turn}
      />
      {all && (
        <Slider
          label="Lower by"
          mixed={on.some((v) => v !== on[0])}
          value={on[0]}
          min={1}
          max={LIMITS.maxDuckDb}
          step={1}
          unit="dB"
          format={(v) => String(Math.round(v))}
          onChange={(v) => editClips(ids, (c) => ({ type: "updateClip", clipId: c.id, duckDb: v }), `${coalesce}:duck`)}
        />
      )}
    </>
  );
}

export function AudioSection({ clip, content }: { clip: Clip; content: Media }) {
  const project = useEditor((s) => s.snap!.project);
  const edit = useEditor((s) => s.edit);
  const maxFade = maxFadeUs(clip.durationUs) / US;
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
      <CleanVoiceRow clips={[clip]} />
      <DuckingRow clips={[clip]} coalesce={clip.id} />
      {isVideo && (
        <Button className="w-full" disabled={!!blocker} disabledReason={blocker ?? undefined} title="Move the sound to its own audio track" onClick={() => detachAudio([clip.id])}>
          <Unlink size={14} /> Detach audio
        </Button>
      )}
    </Section>
  );
}
