import { useEffect, useRef, useState } from "react";
import { AlertTriangle, Check, X } from "lucide-react";
import { api, errorText } from "../lib/api";
import { useEditor } from "../lib/store";
import type { AgentConnection, AgentKind } from "../lib/types";
import { Button, IconButton, trapTab } from "./ui";

const STATE_TEXT: Record<AgentConnection["state"], string> = {
  connected: "Connected",
  other: "Set up for another copy of Nuzky or turned off",
  missing: "Not connected",
  unreadable: "Its settings file could not be read",
};

/** Connect your agent: one click writes Nuzky into Claude Code's or Codex's own settings. */
export function ConnectAgentDialog() {
  const open = useEditor((s) => s.connectOpen);
  const [rows, setRows] = useState<AgentConnection[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<AgentKind | null>(null);
  const dialog = useRef<HTMLDivElement>(null);

  const close = () => useEditor.setState({ connectOpen: false });

  useEffect(() => {
    if (!open) return;
    setError(null);
    setRows(null);
    api.agentConnections().then(setRows, (e) => setError(errorText(e)));
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.stopPropagation();
      close();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [open]);

  const ready = open && rows !== null;
  useEffect(() => {
    if (!ready) return;
    // Closing returns focus to the button that opened the dialog.
    const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    dialog.current?.querySelector<HTMLElement>("[data-autofocus]")?.focus();
    return () => opener?.focus();
  }, [ready]);

  // A connected agent's button goes away, so focus moves on to the next Connect, else to Done.
  const refocus = useRef(false);
  useEffect(() => {
    if (!refocus.current) return;
    refocus.current = false;
    dialog.current?.querySelector<HTMLElement>("[data-autofocus]")?.focus();
  }, [rows]);

  if (!open) return null;

  const connect = async (row: AgentConnection) => {
    setBusy(row.agent);
    setError(null);
    try {
      const done = await api.connectAgent(row.agent);
      refocus.current = !!dialog.current?.contains(document.activeElement);
      setRows((all) => all?.map((r) => (r.agent === done.agent ? done : r)) ?? null);
      useEditor.getState().toast({
        kind: "success",
        text: `${done.name} is connected. Restart ${done.name}, then ask it to edit your video.${done.backup ? ` Your old settings are in ${done.backup}.` : ""}`,
      });
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(null);
    }
  };

  const first = rows?.find((r) => r.state !== "connected" && r.state !== "unreadable");
  return (
    <div
      className="fixed inset-0 z-[100] flex items-center justify-center scrim-in bg-black/45"
      onPointerDown={(e) => e.target === e.currentTarget && close()}
    >
      <div
        ref={dialog}
        role="dialog"
        aria-modal="true"
        aria-labelledby="connect-title"
        onKeyDown={trapTab}
        className="overlay w-[480px] rounded-[20px] p-6 dialog-in"
      >
        <div className="mb-5 flex items-start justify-between gap-3">
          <h2 id="connect-title" className="text-[17px] font-semibold leading-[22px] tracking-[-0.025em]">
            Connect your agent
          </h2>
          <IconButton label="Close" round className="-mr-2 -mt-1" onClick={close}>
            <X size={16} />
          </IconButton>
        </div>
        <div className="flex flex-col gap-3">
          {rows === null && !error && <p className="text-[12px] text-muted">Reading the agents' settings…</p>}
          {rows?.map((row) => (
            <div key={row.agent} role="group" aria-label={row.name} className="flex items-center gap-3 rounded-xl bg-white/[.055] px-3.5 py-3 shadow-[inset_0_0_0_1px_rgb(255_255_255/.06)]">
              <div className="flex min-w-0 flex-1 flex-col gap-0.5">
                <span className="text-[13px] font-medium text-fg">{row.name}</span>
                <span className="truncate text-[11px] text-muted" title={row.path}>
                  {row.path}
                </span>
                <span className={`mt-0.5 flex items-center gap-1 text-[12px] ${row.state === "connected" ? "text-fg" : "text-muted"}`} title={row.problem ?? undefined}>
                  {row.state === "connected" && <Check size={13} className="shrink-0 text-ok" />}
                  {row.state === "unreadable" && <AlertTriangle size={13} className="shrink-0 text-warn" />}
                  {STATE_TEXT[row.state]}
                </span>
              </div>
              {/* A connected agent needs nothing more here; "Connected" says so. */}
              {row.state !== "connected" && (
                <Button
                  pill
                  variant={row === first ? "primary" : undefined}
                  data-autofocus={row === first || undefined}
                  disabled={row.state === "unreadable" || busy !== null}
                  disabledReason={row.state === "unreadable" ? `Fix ${row.path} first; Nuzky does not change a file it cannot read` : "Connecting…"}
                  onClick={() => void connect(row)}
                >
                  {row.state === "other" ? "Reconnect" : "Connect"}
                </Button>
              )}
            </div>
          ))}
          {error && (
            <p className="flex items-start gap-1.5 text-[12px] text-fg" role="alert">
              <AlertTriangle size={14} className="mt-px shrink-0 text-danger" />
              {error}
            </p>
          )}
          <p className="text-[12px] text-muted">
            Only the nuzky entry is added, after a backup of the file. The agent edits the project open here; start it once your project is open.
          </p>
        </div>
        <div className="mt-6 flex justify-end">
          <Button pill variant={rows && !first ? "primary" : undefined} data-autofocus={!first || undefined} onClick={close}>
            Done
          </Button>
        </div>
      </div>
    </div>
  );
}
