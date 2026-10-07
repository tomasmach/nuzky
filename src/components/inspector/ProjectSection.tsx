import { ChevronDown } from "lucide-react";
import { formatLabel } from "../../lib/presets";
import { useEditor } from "../../lib/store";
import { ColorInput, Section, Segmented, Slider, lockedProps, useLockReason } from "../ui";

const DEFAULT_BLUR = 0.5;

export function ProjectSection() {
  const lock = useLockReason();
  const canvas = useEditor((s) => s.snap!.project.canvas);
  const edit = useEditor((s) => s.edit);
  // Size comes from the latest confirmed canvas, so a Ratio change made a moment ago is kept.
  const setCanvas = (patch: { background?: string; backgroundBlur?: number }, key?: string) =>
    edit((p) => ({ type: "setCanvas", width: p.canvas.width, height: p.canvas.height, ...patch }), key);
  const blur = canvas.backgroundBlur > 0;
  return (
    <>
      <Section title="Project">
        <dl className="tabular grid grid-cols-[auto_1fr] items-center gap-x-4 gap-y-1 text-[12px]">
          <dt className="text-muted">Format</dt>
          <dd>
            {/* Opens Ratio under the preview, which stays the one control for the format. */}
            <button
              type="button"
              aria-haspopup="menu"
              title="Change the canvas ratio"
              onClick={() => useEditor.setState({ ratioOpen: true })}
              {...lockedProps(lock)}
              className="-mx-1.5 inline-flex h-6 items-center gap-1 rounded px-1.5 text-fg transition-colors duration-[120ms] ease-out hover:bg-raised aria-disabled:cursor-not-allowed aria-disabled:opacity-40 aria-disabled:hover:bg-transparent"
            >
              {formatLabel(canvas.width, canvas.height)} · {canvas.width}×{canvas.height}
              <ChevronDown size={13} className="text-muted" />
            </button>
          </dd>
          <dt className="text-muted">Frame rate</dt>
          <dd className="text-fg">{canvas.fps} fps</dd>
        </dl>
      </Section>
      <Section title="Background">
        <Segmented
          label="Background"
          value={blur ? "blur" : "color"}
          onChange={(v) => setCanvas({ backgroundBlur: v === "blur" ? DEFAULT_BLUR : 0 })}
          options={[
            { id: "color", label: "Color", title: "Fill empty space with a color" },
            { id: "blur", label: "Blur", title: "Fill empty space with a blurred copy of the video" },
          ]}
        />
        {blur ? (
          <Slider
            label="Strength"
            value={canvas.backgroundBlur * 100}
            min={5}
            max={100}
            step={1}
            unit="%"
            format={(v) => String(Math.round(v))}
            onChange={(v) => setCanvas({ backgroundBlur: v / 100 }, "canvas-blur")}
          />
        ) : (
          <ColorInput label="Color" value={canvas.background} onChange={(v) => setCanvas({ background: v }, "canvas-bg")} />
        )}
      </Section>
    </>
  );
}
