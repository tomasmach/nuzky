import { useEditor } from "../../lib/store";
import { TEXT_PRESETS } from "../../lib/presets";
import { TextSwatch } from "../ui";

export function TextTab() {
  const edit = useEditor((s) => s.edit);
  return (
    <div className="grid grid-cols-2 content-start gap-2 overflow-y-auto p-3">
      {TEXT_PRESETS.map((p) => (
        <button
          key={p.name}
          type="button"
          title={`Add "${p.name}" text at the playhead`}
          onClick={() => edit({ type: "addText", startUs: useEditor.getState().timeUs, text: p.text, style: p.style })}
          className="flex h-16 flex-col items-center justify-center gap-1 rounded-md border border-line bg-line px-2 hover:border-muted"
        >
          <TextSwatch style={p.style} label={p.name === "Title" ? "TITLE" : p.name} />
        </button>
      ))}
    </div>
  );
}
