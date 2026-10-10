import { useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { AlertTriangle, ChevronDown } from "lucide-react";
import { api, errorText, plainError } from "../../lib/api";
import { aiLocked, setBackground, useEditor, useMatteJob } from "../../lib/store";
import type { Background, Clip } from "../../lib/types";
import { IMAGE_EXTENSIONS, importPaths } from "../panel/assets";
import { Button, ColorInput, Menu, ProgressBar, Section, Segmented, Slider, lockedProps, useLockReason } from "../ui";

type Kind = Background["type"];

const DEFAULT_BLUR = 0.5;
const DEFAULT_COLOR = "#1c1c1e";
const NONE: Background = { type: "none" };

const backgroundOf = (clip: Clip): Background => (clip.content.type === "media" ? (clip.content.background ?? NONE) : NONE);

/**
 * Blur or replace what is behind the person in one or more video and image clips. The person is found once per
 * file in the background; meanwhile the preview shows the clips as recorded and the section says how far it is.
 */
export function BackgroundSection({ clips }: { clips: Clip[] }) {
  const lock = useLockReason();
  const project = useEditor((s) => s.snap!.project);
  const toast = useEditor((s) => s.toast);
  const ids = clips.map((c) => c.id);
  const backgrounds = clips.map(backgroundOf);
  const first = backgrounds[0] ?? NONE;
  const kind: Kind | null = backgrounds.every((b) => b.type === first.type) ? first.type : null;
  const own = new Set(clips.flatMap((c) => (c.content.type === "media" ? [c.content.assetId] : [])));
  // A picture is not put behind itself.
  const images = project.assets.filter((a) => a.kind === "image" && !own.has(a.id));
  const paths = clips.flatMap((c) => {
    if (c.content.type !== "media" || backgroundOf(c).type === "none") return [];
    const assetId = c.content.assetId;
    return project.assets.filter((a) => a.id === assetId).map((a) => a.path);
  });
  const job = useMatteJob(paths);
  const on = backgrounds.some((b) => b.type !== "none");
  const models = usePersonModel();
  const waiting = useWaiting(paths, on && !!models?.downloaded && !job);
  const [menu, setMenu] = useState<{ x: number; y: number; keyboard: boolean } | null>(null);
  const picker = useRef<HTMLButtonElement>(null);

  const set = (background: Background, coalesce?: string) => setBackground(ids, background, coalesce);
  const importImage = async () => {
    if (aiLocked()) return;
    const picked = await open({ multiple: false, filters: [{ name: "Images", extensions: IMAGE_EXTENSIONS }] });
    if (typeof picked !== "string") return;
    const [assetId] = await importPaths([picked]);
    if (assetId) set({ type: "image", assetId });
  };
  const choose = (next: Kind) => {
    // Choosing again keeps the strength, colour or image already set.
    if (next === kind) return;
    if (next === "none") set(NONE);
    else if (next === "blur") set({ type: "blur", strength: DEFAULT_BLUR });
    else if (next === "color") set({ type: "color", color: DEFAULT_COLOR });
    else if (images.length > 0) set({ type: "image", assetId: images[images.length - 1].id });
    else void importImage();
  };
  const download = () => api.startVisionModels(false, "background").catch((e) => toast({ kind: "error", text: errorText(e) }));

  const picture = first.type === "image" ? project.assets.find((a) => a.id === first.assetId) : undefined;
  const samePicture = backgrounds.every((b) => b.type === "image" && first.type === "image" && b.assetId === first.assetId);
  const openMenu = (keyboard: boolean) => {
    const r = picker.current?.getBoundingClientRect();
    if (r) setMenu({ x: r.right, y: r.bottom + 4, keyboard });
  };
  const unavailable = models?.unavailable ? plainError(models.unavailable) : null;
  const thumb = useEditor((s) => (picture ? s.thumbs[picture.id] : null));

  return (
    <Section title="Background">
      <Segmented
        label="Background"
        value={kind}
        onChange={choose}
        disabled={!!unavailable && !on}
        disabledReason={unavailable ?? undefined}
        options={[
          { id: "none", label: "None", title: "The picture as recorded" },
          { id: "blur", label: "Blur", title: "Blur what is behind the person" },
          { id: "color", label: "Color", title: "Put a color behind the person" },
          { id: "image", label: "Image", title: "Put an image behind the person" },
        ]}
      />
      {kind === "blur" && first.type === "blur" && (
        <Slider
          label="Strength"
          mixed={backgrounds.some((b) => b.type !== "blur" || b.strength !== first.strength)}
          value={first.strength * 100}
          min={5}
          max={100}
          step={1}
          unit="%"
          format={(v) => String(Math.round(v))}
          onChange={(v) => set({ type: "blur", strength: v / 100 }, `${ids.join(",")}:background-blur`)}
        />
      )}
      {kind === "color" && first.type === "color" && (
        <ColorInput
          label="Color"
          value={first.color}
          mixed={backgrounds.some((b) => b.type !== "color" || b.color !== first.color)}
          onChange={(color) => set({ type: "color", color }, `${ids.join(",")}:background-color`)}
        />
      )}
      {kind === "image" && (
        <div className="flex items-center justify-between gap-2 text-[12px]">
          <span className="text-muted">Image</span>
          <button
            ref={picker}
            type="button"
            aria-haspopup="menu"
            aria-expanded={!!menu}
            title="Choose the image behind the person"
            aria-label={`Image behind the person: ${samePicture ? (picture?.name ?? "missing") : "several"}`}
            onClick={(e) => openMenu(e.detail === 0)}
            {...lockedProps(lock)}
            className="-mr-1.5 inline-flex h-6 min-w-0 items-center gap-1 rounded-md px-1.5 text-fg transition-colors duration-[120ms] ease-out hover:bg-white/[.08] aria-disabled:cursor-not-allowed aria-disabled:opacity-40 aria-disabled:hover:bg-transparent"
          >
            {samePicture && thumb && <img src={thumb} alt="" className="h-5 w-8 shrink-0 rounded-[5px] object-cover shadow-[inset_0_0_0_1px_rgb(255_255_255/.18)]" />}
            <span className="truncate">{samePicture ? (picture?.name ?? "Missing image") : "—"}</span>
            <ChevronDown size={13} className="shrink-0 text-muted" />
          </button>
        </div>
      )}
      {menu && (
        <Menu
          label="Image behind the person"
          at={{ x: menu.x, y: menu.y, align: "end" }}
          keyboard={menu.keyboard}
          onClose={() => {
            setMenu(null);
            requestAnimationFrame(() => picker.current?.focus());
          }}
          items={[
            ...images.map((a) => ({ label: a.name, checked: samePicture && first.type === "image" && first.assetId === a.id, run: () => set({ type: "image", assetId: a.id }) })),
            ...(images.length > 0 ? (["separator"] as const) : []),
            { label: "Import image…", run: () => void importImage() },
          ]}
        />
      )}
      {on && unavailable && (
        <p role="status" className="flex items-start gap-1.5 text-[12px] text-muted">
          <AlertTriangle size={14} className="mt-px shrink-0 text-warn" />
          {unavailable}
        </p>
      )}
      {on && !unavailable && models && !models.downloaded && (
        <div role="status" className="flex items-center justify-between gap-2 text-[12px] text-muted">
          {models.downloading !== null ? (
            <div className="flex flex-1 flex-col gap-1.5">
              <span className="tabular">Downloading the person model{models.downloading > 0 && ` · ${Math.round(models.downloading * 100)}%`}</span>
              <ProgressBar value={models.downloading} label="Downloading the person model" />
            </div>
          ) : (
            <>
              <span>Showing and exporting it needs the person model ({models.sizeMb} MB)</span>
              <Button className="h-7 px-2.5 text-[12px]" onClick={download}>
                Download
              </Button>
            </>
          )}
        </div>
      )}
      {job?.progress != null && (
        <div className="flex flex-col gap-1.5" role="status">
          <span className="tabular text-[12px] text-muted">Finding the person{job.progress > 0 && ` · ${Math.round(job.progress * 100)}%`}</span>
          <ProgressBar value={job.progress} label="Finding the person" />
        </div>
      )}
      {waiting && (
        <p role="status" className="text-[12px] text-muted">
          Waiting to find the person
        </p>
      )}
      {job?.failed != null && (
        <p role="status" className="flex items-start gap-1.5 text-[12px] text-muted">
          <AlertTriangle size={14} className="mt-px shrink-0 text-danger" />
          {`The person could not be found: ${plainError(job.failed).split(": ").pop()}. The clip shows as recorded; choose None to export it.`}
        </p>
      )}
    </Section>
  );
}

/** Whether these files wait for the person to be found while no job runs for them yet, as when another file is first. */
function useWaiting(paths: string[], wanted: boolean) {
  const [missing, setMissing] = useState<string[]>([]);
  const finished = useEditor((s) => Object.values(s.jobs).filter((j) => j.kind === "matte" && j.status !== "running").length);
  const key = paths.join("\n");
  useEffect(() => {
    if (!wanted) return;
    let current = true;
    api
      .mattesMissing()
      .then((m) => current && setMissing(m))
      .catch(() => current && setMissing([]));
    return () => {
      current = false;
    };
  }, [wanted, key, finished]);
  return wanted && paths.some((p) => missing.includes(p));
}

/** Whether the person model can run and is installed, and how far its download is; checked again after one. */
function usePersonModel() {
  const [status, setStatus] = useState<{ sizeMb: number; downloaded: boolean; unavailable: string | null } | null>(null);
  const downloading = useEditor((s) => {
    const job = Object.values(s.jobs).find((j) => j.id.startsWith("vision-models:") && j.status === "running");
    return job ? job.progress : null;
  });
  const busy = downloading !== null;
  useEffect(() => {
    if (busy) return;
    let current = true;
    api
      .visionModels("background")
      .then((s) => current && setStatus(s))
      .catch(() => current && setStatus(null));
    return () => {
      current = false;
    };
  }, [busy]);
  return status && { ...status, downloading };
}
