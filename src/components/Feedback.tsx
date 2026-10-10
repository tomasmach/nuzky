import { useState, type MouseEvent } from "react";
import { AtSign, Bug, Lightbulb, MessageSquare } from "lucide-react";
import { api, errorText, type FeedbackKind } from "../lib/api";
import { useEditor } from "../lib/store";
import { IconButton, Menu, type MenuEntry } from "./ui";

type ShowMenu = (items: MenuEntry[], at: { x: number; y: number; above?: boolean }, label: string, keyboard: boolean) => void;

function open(kind: FeedbackKind) {
  api.openFeedback(kind).catch((e) => useEditor.getState().toast({ kind: "error", text: `Couldn't open the browser: ${errorText(e)}` }));
}

/** Each opens a page in the browser; a GitHub issue comes prefilled with this version and system, and nothing is sent until the user submits it there. */
const feedbackMenu = (): MenuEntry[] => [
  { label: "Report a bug…", icon: <Bug size={15} />, run: () => open("bug") },
  { label: "Share an idea…", icon: <Lightbulb size={15} />, run: () => open("idea") },
  "separator",
  { label: "Message me on X…", icon: <AtSign size={15} />, run: () => open("message") },
];

export function FeedbackRow({ showMenu }: { showMenu: ShowMenu }) {
  const openMenu = (e: MouseEvent<HTMLButtonElement>) => {
    const r = e.currentTarget.getBoundingClientRect();
    showMenu(feedbackMenu(), { x: r.left, y: r.top - 6, above: true }, "Send feedback", e.detail === 0);
  };
  return (
    <button
      type="button"
      data-feedback
      aria-haspopup="menu"
      onClick={openMenu}
      className="flex h-8 w-full shrink-0 items-center gap-2.5 rounded-lg px-2.5 text-left text-[13px] text-muted transition-colors duration-[120ms] ease-out hover:bg-white/[.05] hover:text-fg"
    >
      <MessageSquare size={16} className="shrink-0" /> Send feedback
    </button>
  );
}

export function FeedbackButton() {
  const [menu, setMenu] = useState<{ at: { x: number; y: number }; keyboard: boolean; back: HTMLElement } | null>(null);
  return (
    <div className="bar flex rounded-full">
      <IconButton
        round
        data-feedback
        aria-haspopup="menu"
        aria-expanded={!!menu}
        label="Send feedback"
        onClick={(e) => {
          const r = e.currentTarget.getBoundingClientRect();
          setMenu({ at: { x: r.left, y: r.bottom + 6 }, keyboard: e.detail === 0, back: e.currentTarget });
        }}
      >
        <MessageSquare size={16} />
      </IconButton>
      {menu && (
        <Menu
          items={feedbackMenu()}
          at={menu.at}
          label="Send feedback"
          keyboard={menu.keyboard}
          onClose={() => {
            menu.back.focus();
            setMenu(null);
          }}
        />
      )}
    </div>
  );
}
