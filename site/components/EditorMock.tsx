import Image from "next/image";
import {
  BetweenHorizontalEnd,
  BetweenHorizontalStart,
  Captions,
  Clapperboard,
  Copy,
  Download,
  Eye,
  LayoutGrid,
  Magnet,
  Music2,
  Play,
  Redo2,
  Scan,
  Scissors,
  SkipBack,
  SkipForward,
  Sparkles,
  Trash2,
  Type,
  Undo2,
  Volume2,
  ZoomIn,
  ZoomOut,
} from "lucide-react";

// A still of the Nuzky editor at 1200 × 740, drawn from the app's tokens in DESIGN.md.
// `focus` dims everything except the part the current showcase tab talks about.

export type EditorFocus = "edit" | "captions" | "agent" | "export";

const tiles = ["tile-1", "tile-2", "tile-3", "tile-4", "tile-5", "tile-6", "reel-concert", "reel-coffee", "strip-5"];
const durations = ["0:12", "0:34", "1:08", "0:07", "0:21", "0:45", "0:16", "0:09", "0:52"];

function Pane({ className = "", children }: { className?: string; children: React.ReactNode }) {
  return <div className={`overflow-hidden rounded-xl bg-panel ring-1 ring-inset ring-white/[0.06] ${className}`}>{children}</div>;
}

function Capsule({ className = "", children }: { className?: string; children: React.ReactNode }) {
  return <div className={`capsule flex h-[30px] items-center gap-1.5 rounded-full px-3 text-[12px] font-medium ${className}`}>{children}</div>;
}

function Slider({ label, value, text }: { label: string; value: number; text: string }) {
  return (
    <div className="flex items-center gap-2.5">
      <span className="w-[58px] text-[12px] text-muted">{label}</span>
      <div className="relative h-3.5 w-[92px]">
        <div className="absolute inset-x-0 top-[5px] h-1 rounded-full bg-white/[0.13]" />
        <div className="absolute left-0 top-[5px] h-1 rounded-full bg-accent" style={{ width: value }} />
        <div className="absolute top-0 h-3.5 w-[22px] rounded-full bg-white shadow-[0_1px_3px_rgb(0_0_0/0.4)]" style={{ left: value - 11 }} />
      </div>
      <span className="flex h-6 w-[62px] items-center justify-end whitespace-nowrap rounded-md bg-white/[0.055] px-[7px] text-[12px] tabular-nums ring-1 ring-inset ring-white/[0.08]">{text}</span>
    </div>
  );
}

function Clip({ x, w, top, h, color, label, icon, selected, image }: { x: number; w: number; top: number; h: number; color: string; label: string; icon?: React.ReactNode; selected?: boolean; image?: string }) {
  return (
    <div
      className={`absolute overflow-hidden rounded-[7px] ${selected ? "ring-2 ring-accent" : "ring-1 ring-inset ring-white/10"}`}
      style={{ left: x, width: w - 2, top, height: h, background: color }}
    >
      {image && <Image src={image} alt="" fill sizes="320px" className="object-cover" />}
      <span className="absolute left-1.5 top-1 flex items-center gap-1 rounded bg-black/60 px-[5px] py-px text-[10px] font-medium">
        {icon}
        {label}
      </span>
    </div>
  );
}

export function EditorMock({ focus = "edit" }: { focus?: EditorFocus }) {
  const dim = (on: boolean) => `transition-opacity duration-300 ${on ? "opacity-100" : "opacity-35"}`;
  const lanes = { captions: 26, title: 58, video: 90, audio: 152 };
  const bars = Array.from({ length: 180 }, (_, k) => 2 + Math.round(10 * Math.abs(Math.sin(k * 0.37) * Math.cos(k * 0.11))));

  return (
    <div className="flex h-[740px] w-[1200px] flex-col gap-1.5 overflow-hidden rounded-[14px] bg-bg px-1.5 pb-1.5 text-left text-fg ring-1 ring-white/10">
      {/* The macOS title bar: window controls on the left, the project in the middle. */}
      <div className="relative -mx-1.5 flex h-7 shrink-0 items-center border-b border-white/[0.06] bg-white/[0.025] px-3.5">
        <div className="flex items-center gap-2">
          {["#ff5f57", "#febc2e", "#28c840"].map((c) => (
            <span key={c} className="size-3 rounded-full ring-[0.5px] ring-black/20" style={{ background: c }} />
          ))}
        </div>
        <div className="absolute inset-x-0 flex justify-center gap-2 text-[12px]">
          <span className="font-semibold">Morning run</span>
          <span className="text-muted">Saved</span>
        </div>
      </div>

      {/* Top bar */}
      <div className="flex h-11 shrink-0 items-center justify-between">
        <Capsule>
          <LayoutGrid className="size-[15px] text-muted" />
          Projects
        </Capsule>
        <div className="flex w-[260px] items-center justify-end gap-2">
          <Capsule className={dim(focus !== "export")}>
            <Sparkles className="size-[15px] text-accent" />
            AI
          </Capsule>
          <Capsule className={dim(focus !== "export")}>
            <Undo2 className="size-[15px] text-muted" />
            <Redo2 className="size-[15px] text-subtle" />
          </Capsule>
          <div className="flex h-[30px] items-center gap-1.5 rounded-full bg-accent-strong pl-3 pr-3.5 text-[12px] font-medium text-white shadow-[inset_0_1px_0_rgb(255_255_255/0.15)]">
            <Download className="size-3.5" />
            Export
          </div>
        </div>
      </div>

      <div className="flex min-h-0 flex-1 gap-1.5">
        {/* Library */}
        <Pane className={`flex w-[300px] shrink-0 flex-col gap-3 p-2.5 ${dim(focus === "edit")}`}>
          <div className="flex gap-0.5 rounded-[9px] bg-white/[0.045] p-0.5">
            {[
              [Clapperboard, "Media"],
              [Music2, "Audio"],
              [Type, "Text"],
              [Captions, "Captions"],
              [Sparkles, "Effects"],
            ].map(([Icon, t], i) => {
              const I = Icon as typeof Clapperboard;
              return (
                <div key={t as string} className={`flex flex-1 flex-col items-center gap-[3px] rounded-[7px] py-1.5 ${i === 0 ? "bg-white/15" : ""}`}>
                  <I className={`size-3.5 ${i === 0 ? "text-accent" : "text-muted"}`} />
                  <span className={`text-[10px] ${i === 0 ? "text-fg" : "text-muted"}`}>{t as string}</span>
                </div>
              );
            })}
          </div>
          <span className="text-[12px] font-semibold">Imported</span>
          <div className="grid grid-cols-3 gap-2">
            {tiles.map((t, i) => (
              <div key={t} className="relative h-[84px] overflow-hidden rounded-[10px] bg-raised">
                <Image src={`/media/${t}.jpg`} alt="" fill sizes="180px" className="object-cover" />
                <span className="absolute bottom-[5px] right-[5px] rounded-[5px] bg-black/60 px-[5px] py-0.5 text-[10px] font-medium tabular-nums">{durations[i]}</span>
              </div>
            ))}
          </div>
        </Pane>

        {/* Preview */}
        <div className="flex min-w-0 flex-1 flex-col items-center justify-center gap-3.5 rounded-xl bg-stage p-3.5 ring-1 ring-inset ring-white/[0.06]">
          <div className="relative h-[340px] w-[191px] ring-1 ring-white/[0.12]">
            <Image src="/media/reel-hike.jpg" alt="" fill sizes="400px" className="object-cover" />
            <div className={`absolute inset-x-0 bottom-16 flex justify-center ${focus === "captions" ? "" : ""}`}>
              <span
                className={`flex gap-[5px] rounded-md bg-black/70 px-2 py-1 text-[15px] font-extrabold transition-shadow duration-300 ${focus === "captions" ? "ring-2 ring-accent" : ""}`}
              >
                first light <span className="text-caption-yellow">on the ridge</span>
              </span>
            </div>
          </div>
          <div className="capsule flex h-10 w-full items-center justify-between rounded-full px-3.5">
            <span className="w-[130px] text-[12px] font-medium tabular-nums">
              00:04.12 <span className="font-normal text-muted">/ 00:31.00</span>
            </span>
            <div className="flex items-center gap-3.5">
              <SkipBack className="size-3.5" />
              <span className="flex size-7 items-center justify-center rounded-full bg-fg text-bg">
                <Play className="size-[13px] fill-current" />
              </span>
              <SkipForward className="size-3.5" />
            </div>
            <span className="flex w-[130px] items-center justify-end gap-2.5 text-[12px] text-muted">
              <Scan className="size-3.5" />
              9:16
            </span>
          </div>
        </div>

        {/* Inspector */}
        <Pane className={`flex w-[260px] shrink-0 flex-col gap-3.5 p-3 ${dim(focus === "edit" || focus === "captions")}`}>
          <span className="text-[15px] font-semibold">Video</span>
          <div className="flex rounded-[9px] bg-white/[0.045] p-0.5">
            {["Basic", "Audio", "Speed"].map((t, i) => (
              <span key={t} className={`flex-1 rounded-[7px] py-[5px] text-center text-[12px] font-medium ${i === 0 ? "bg-white/15" : "text-muted"}`}>
                {t}
              </span>
            ))}
          </div>
          <Slider label="Scale" value={62} text="112%" />
          <Slider label="Opacity" value={86} text="90%" />
          <Slider label="Rotation" value={50} text="0°" />
          <Slider label="Volume" value={72} text="−2.0 dB" />
          <Slider label="Speed" value={33} text="1.0×" />
          <span className="text-[13px] font-semibold">Caption style</span>
          <div className="flex gap-2">
            {[
              ["Bold", "text-caption-yellow", true],
              ["Clean", "text-fg", false],
              ["Pop", "text-accent", false],
            ].map(([n, c, on]) => (
              <span key={n as string} className={`flex h-[52px] flex-1 items-center justify-center rounded-[10px] bg-raised text-[14px] font-extrabold ${c} ${on ? "ring-2 ring-accent" : "ring-1 ring-inset ring-white/[0.06]"}`}>
                {n}
              </span>
            ))}
          </div>
        </Pane>
      </div>

      {/* Timeline */}
      <Pane className="flex h-[244px] shrink-0 flex-col">
        <div className="flex h-11 items-center justify-between px-2.5">
          <div className={`capsule flex h-[30px] items-center gap-3.5 rounded-full px-3 ${dim(focus === "edit")}`}>
            {[Scissors, BetweenHorizontalStart, BetweenHorizontalEnd, Trash2, Copy].map((I, i) => (
              <I key={i} className="size-3.5" />
            ))}
            <span className="h-3.5 w-px bg-white/15" />
            <Magnet className="size-3.5 text-accent" />
          </div>
          <div className="flex items-center gap-2.5 text-muted">
            <ZoomOut className="size-3.5" />
            <div className="relative h-3 w-[90px]">
              <div className="absolute inset-x-0 top-1 h-1 rounded-full bg-white/[0.13]" />
              <div className="absolute left-0 top-1 h-1 w-10 rounded-full bg-accent" />
              <div className="absolute left-[34px] top-0 size-3 rounded-full bg-white" />
            </div>
            <ZoomIn className="size-3.5" />
          </div>
        </div>
        <div className="flex min-h-0 flex-1">
          <div className="flex w-[84px] shrink-0 flex-col pt-[22px] text-muted">
            {[
              [<Captions key="c" className="size-[13px]" />, 32],
              [<Type key="t" className="size-[13px]" />, 32],
              [
                <span key="v" className="flex items-center gap-2">
                  <Clapperboard className="size-[13px]" />
                  <Eye className="size-[13px]" />
                  <Volume2 className="size-[13px]" />
                </span>,
                62,
              ],
              [<Music2 key="m" className="size-[13px]" />, 40],
            ].map(([icon, h], i) => (
              <div key={i} className="flex items-center px-3" style={{ height: h as number }}>
                {icon}
              </div>
            ))}
          </div>
          <div className="relative min-w-0 flex-1 overflow-hidden">
            {Array.from({ length: 10 }, (_, s) => (
              <div key={s} className="absolute top-0.5" style={{ left: s * 110 }}>
                <span className="block pl-1 text-[10px] tabular-nums text-muted">00:{String(s * 3).padStart(2, "0")}</span>
                <span className="mt-0.5 block h-1.5 w-px bg-white/20" />
              </div>
            ))}
            <div className={dim(focus === "captions" || focus === "edit")}>
              {[
                [0, 140, "first light"],
                [150, 150, "on the ridge"],
                [310, 130, "we made it"],
                [450, 190, "to the top"],
                [650, 150, "worth it"],
              ].map(([x, w, l]) => (
                <Clip key={l as string} x={x as number} w={w as number} top={lanes.captions} h={26} color="var(--color-clip-captions)" label={l as string} />
              ))}
            </div>
            <div className={dim(focus === "edit")}>
              <Clip x={110} w={260} top={lanes.title} h={26} color="var(--color-clip-title)" label="Morning run" icon={<Type className="size-2.5" />} />
            </div>
            <div className={dim(focus !== "captions")}>
              {[
                [0, 250, "IMG_2041.MOV", "clip-a"],
                [250, 320, "IMG_2044.MOV", "strip-4"],
                [570, 230, "IMG_2047.MOV", "strip-2"],
                [800, 300, "IMG_2052.MOV", "strip-6"],
              ].map(([x, w, l, img], i) => (
                <Clip
                  key={l}
                  x={x as number}
                  w={w as number}
                  top={lanes.video}
                  h={56}
                  color="var(--color-clip-video)"
                  label={l as string}
                  image={`/media/${img}.jpg`}
                  selected={i === 1}
                  icon={<Clapperboard className="size-2.5" />}
                />
              ))}
              <Clip x={0} w={1100} top={lanes.audio} h={34} color="var(--color-clip-audio)" label="Sunrise — ambient" icon={<Music2 className="size-2.5" />} />
              <div className="absolute left-1.5 flex items-center gap-0.5" style={{ top: lanes.audio + 20, height: 12 }}>
                {bars.map((h, k) => (
                  <span key={k} className="w-1 shrink-0 rounded-[1px] bg-ok/60" style={{ height: h }} />
                ))}
              </div>
            </div>
            <span className="absolute top-4 h-[200px] w-[1.5px] bg-white" style={{ left: 398 }} />
            <span className="absolute top-3 size-[11px] rounded-full bg-white" style={{ left: 393 }} />
          </div>
        </div>
      </Pane>
    </div>
  );
}
