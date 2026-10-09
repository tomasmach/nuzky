import { useState, type MouseEvent } from "react";
import { CircleArrowDown, Loader2 } from "lucide-react";
import { checkForUpdates, downloadUpdate, setAutoCheck, useUpdates } from "../lib/updates";
import { IconButton, Menu, type MenuEntry } from "./ui";

type ShowMenu = (items: MenuEntry[], at: { x: number; y: number; above?: boolean }, label: string, keyboard: boolean) => void;

/** Download when there is something newer, a check now, and the automatic check on or off. */
function updateMenu(): MenuEntry[] {
  const { latest, checking, auto } = useUpdates.getState();
  return [
    ...(latest ? [{ label: `Download CapOpen ${latest}…`, icon: <CircleArrowDown size={15} />, run: downloadUpdate }, "separator" as const] : []),
    { label: "Check for updates", disabled: checking ? "Checking for updates…" : null, run: () => void checkForUpdates(true) },
    { label: "Check automatically", checked: auto, run: () => setAutoCheck(!auto) },
  ];
}

/**
 * The foot of the home sidebar: this version, or the newer one to download in an accent row. Its
 * menu checks for updates and turns the automatic check on or off. CapOpen starts on the home
 * screen, so a new version is seen at the next launch.
 */
export function VersionRow({ showMenu }: { showMenu: ShowMenu }) {
  const { version, enabled, latest, checking } = useUpdates();
  if (!version) return null;
  if (!enabled) return <p className="flex h-8 items-center px-2.5 text-[12px] text-muted">CapOpen {version}</p>;

  const open = (e: MouseEvent<HTMLButtonElement>) => {
    const r = e.currentTarget.getBoundingClientRect();
    showMenu(updateMenu(), { x: r.left, y: r.top - 6, above: true }, "Updates", e.detail === 0);
  };
  const found = !!latest && checking !== "manual";

  return (
    <button
      type="button"
      data-version
      aria-haspopup="menu"
      title={latest ? `CapOpen ${latest} is available. You have ${version}.` : "Updates"}
      onClick={open}
      className={`flex w-full shrink-0 items-center gap-2.5 rounded-lg px-2.5 text-left text-[13px] transition-colors duration-[120ms] ease-out ${
        found ? "h-9 bg-accent/[.16] shadow-[inset_0_0_0_1px_rgb(41_151_255/.45)] hover:bg-accent/[.22]" : "h-8 hover:bg-white/[.05]"
      }`}
    >
      {checking === "manual" ? (
        <>
          <Loader2 size={16} className="shrink-0 animate-spin text-muted" />
          <span className="min-w-0 flex-1 truncate text-muted">Checking for updates…</span>
        </>
      ) : found ? (
        <>
          <CircleArrowDown size={16} className="shrink-0 text-accent" />
          <span className="min-w-0 flex-1 truncate font-semibold text-fg">Update available</span>
          <span className="tabular shrink-0 text-[12px] font-medium text-accent">{latest}</span>
        </>
      ) : (
        <span className="min-w-0 flex-1 truncate text-[12px] text-muted">CapOpen {version}</span>
      )}
    </button>
  );
}

/** In the editor's top bar, only while a newer version is out: a quiet button with the same menu. */
export function UpdateButton() {
  const latest = useUpdates((s) => (s.enabled ? s.latest : null));
  const [menu, setMenu] = useState<{ at: { x: number; y: number }; keyboard: boolean; back: HTMLElement } | null>(null);
  if (!latest) return null;
  return (
    <div className="bar flex rounded-full">
      <IconButton
        round
        data-update-button
        aria-haspopup="menu"
        aria-expanded={!!menu}
        label={`CapOpen ${latest} is available`}
        onClick={(e) => {
          const r = e.currentTarget.getBoundingClientRect();
          setMenu({ at: { x: r.left, y: r.bottom + 6 }, keyboard: e.detail === 0, back: e.currentTarget });
        }}
      >
        <CircleArrowDown size={16} className="text-accent" />
      </IconButton>
      {menu && (
        <Menu
          items={updateMenu()}
          at={menu.at}
          label="Updates"
          keyboard={menu.keyboard}
          onClose={(chose) => {
            const back = menu.back;
            setMenu(null);
            if (!chose) back.focus();
            // After Download the button may be gone; otherwise focus goes back to it, not to the page.
            else requestAnimationFrame(() => document.activeElement === document.body && back.isConnected && back.focus());
          }}
        />
      )}
    </div>
  );
}
