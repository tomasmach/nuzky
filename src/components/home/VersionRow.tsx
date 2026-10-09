import type { MouseEvent } from "react";
import { CircleArrowDown, Loader2 } from "lucide-react";
import { checkForUpdates, downloadUpdate, setAutoCheck, useUpdates } from "../../lib/updates";
import type { MenuEntry } from "../ui";

type ShowMenu = (items: MenuEntry[], at: { x: number; y: number; above?: boolean }, label: string, keyboard: boolean) => void;

/**
 * The foot of the home sidebar: this version, or the newer one to download. Its menu checks for
 * updates and turns the automatic check on or off. CapOpen starts on the home screen, so a new
 * version is seen at the next launch without anything appearing over the editor.
 */
export function VersionRow({ showMenu }: { showMenu: ShowMenu }) {
  const { version, enabled, auto, latest, checking } = useUpdates();
  if (!version) return null;
  if (!enabled) return <p className="flex h-8 items-center px-2.5 text-[12px] text-muted">CapOpen {version}</p>;

  const open = (e: MouseEvent<HTMLButtonElement>) => {
    const r = e.currentTarget.getBoundingClientRect();
    showMenu(
      [
        ...(latest ? [{ label: `Download CapOpen ${latest}…`, icon: <CircleArrowDown size={15} />, run: downloadUpdate }, "separator" as const] : []),
        { label: "Check for updates", disabled: checking ? "Checking for updates…" : null, run: () => void checkForUpdates(true) },
        { label: "Check automatically", checked: auto, run: () => setAutoCheck(!auto) },
      ],
      { x: r.left, y: r.top - 6, above: true },
      "Updates",
      e.detail === 0,
    );
  };

  return (
    <button
      type="button"
      data-version
      aria-haspopup="menu"
      title={latest ? `CapOpen ${latest} is available. You have ${version}.` : "Updates"}
      onClick={open}
      className="flex h-8 w-full shrink-0 items-center gap-2.5 rounded-lg px-2.5 text-left text-[13px] transition-colors duration-[120ms] ease-out hover:bg-white/[.05]"
    >
      {checking === "manual" ? (
        <>
          <Loader2 size={16} className="shrink-0 animate-spin text-muted" />
          <span className="min-w-0 flex-1 truncate text-muted">Checking for updates…</span>
        </>
      ) : latest ? (
        <>
          <CircleArrowDown size={16} className="shrink-0 text-accent" />
          <span className="min-w-0 flex-1 truncate font-medium text-fg">Update available</span>
          <span className="tabular shrink-0 text-[12px] text-muted">{latest}</span>
        </>
      ) : (
        <span className="min-w-0 flex-1 truncate text-[12px] text-muted">CapOpen {version}</span>
      )}
    </button>
  );
}
