import { useEditor } from "../../lib/store";
import { TEXT_PRESETS } from "../../lib/presets";
import { TextSwatch, lockedProps, useLockReason } from "../ui";

export function TextTab() {
  const edit = useEditor((s) => s.edit);
  const lock = useLockReason();
  return (
    <div className="grid grid-cols-[repeat(auto-fill,minmax(150px,1fr))] content-start gap-2 overflow-y-auto p-3.5">
      {TEXT_PRESETS.map((p) => (
        <button
          key={p.name}
          type="button"
          title={`Add "${p.name}" text at the playhead`}
          onClick={() => edit({ type: "addText", startUs: useEditor.getState().timeUs, text: p.text, style: p.style })}
          {...lockedProps(lock)}
          className="flex h-16 min-w-0 items-center justify-center rounded-[10px] border border-white/[.08] bg-white/[.1] px-2 transition-colors duration-[120ms] ease-out hover:border-white/30 aria-disabled:cursor-not-allowed aria-disabled:opacity-40 aria-disabled:hover:border-white/[.08]"
        >
          <TextSwatch style={p.style} label={p.name === "Title" ? "TITLE" : p.name} />
        </button>
      ))}
    </div>
  );
}
