import { formatLabel } from "../../lib/presets";
import { useEditor } from "../../lib/store";
import { ColorInput, Section, Segmented, Slider } from "../ui";

const DEFAULT_BLUR = 0.5;

export function ProjectSection() {
  const canvas = useEditor((s) => s.snap!.project.canvas);
  const edit = useEditor((s) => s.edit);
  const setCanvas = (patch: { background?: string; backgroundBlur?: number }, key?: string) =>
    edit({ type: "setCanvas", width: canvas.width, height: canvas.height, ...patch }, key);
  const blur = canvas.backgroundBlur > 0;
  return (
    <>
      <Section title="Project">
        <dl className="tabular grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-[12px]">
          <dt className="text-muted">Format</dt>
          <dd className="text-fg">
            {formatLabel(canvas.width, canvas.height)} · {canvas.width}×{canvas.height}
          </dd>
          <dt className="text-muted">Frame rate</dt>
          <dd className="text-fg">{canvas.fps} fps</dd>
        </dl>
        <p className="text-[12px] text-muted">Change the format with Ratio under the preview.</p>
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
