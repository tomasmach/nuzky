import { useEffect, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import {
  AlertCircle,
  AlertTriangle,
  ArrowUp,
  Captions,
  Check,
  CheckCircle2,
  ChevronDown,
  ChevronRight,
  CircleStop,
  Crosshair,
  Download,
  ExternalLink,
  Film,
  GripVertical,
  ImagePlus,
  Info,
  Loader2,
  Plug,
  RefreshCw,
  Scissors,
  SquarePen,
  Undo2,
  X,
} from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  NOT_READY,
  READY,
  choose,
  chooseAgent,
  markUndone,
  loadAgents,
  newChat,
  send,
  sendBlocked,
  stop,
  useAgent,
  type AgentErrorCode,
  type AgentId,
  type ChatItem,
  type RunChange,
  type Step,
} from "../../lib/agent";
import { DOCK_LABELS, TOO_NARROW, useDock, useDockLayout, type DockMode } from "../../lib/dock";
import { undoAction, undoStep, useEditor } from "../../lib/store";
import { formatTime } from "../../lib/time";
import { Button, IconButton } from "../ui";
import { Markdown } from "./Markdown";
import { moveFloatBy, startMove } from "./AiDock";

const INSTALL: Record<AgentId, string> = {
  claude: "https://docs.claude.com/en/docs/claude-code/setup",
  codex: "https://developers.openai.com/codex/cli",
};
const COMMAND: Record<AgentId, string> = { claude: "claude", codex: "codex" };

/** A small menu under a trigger: ↑/↓ move, Esc closes back to the trigger, no key reaches the editor. */
function Menu({
  label,
  trigger,
  children,
}: {
  label: string;
  trigger: (p: { open: boolean; toggle: () => void; ref: React.RefObject<HTMLButtonElement | null> }) => ReactNode;
  children: (close: () => void) => ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const close = () => {
    setOpen(false);
    triggerRef.current?.focus();
  };
  useEffect(() => {
    if (!open) return;
    const items = [...(ref.current?.querySelectorAll<HTMLElement>("[role^=menuitem]") ?? [])];
    (items.find((el) => el.getAttribute("aria-checked") === "true") ?? items[0])?.focus();
    const outside = (e: PointerEvent) => !ref.current?.contains(e.target as Node) && setOpen(false);
    const esc = (e: globalThis.KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.stopPropagation();
      close();
    };
    window.addEventListener("pointerdown", outside);
    window.addEventListener("keydown", esc, true);
    return () => {
      window.removeEventListener("pointerdown", outside);
      window.removeEventListener("keydown", esc, true);
    };
  }, [open]);
  const onKey = (e: KeyboardEvent<HTMLDivElement>) => {
    e.stopPropagation();
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    e.preventDefault();
    const items = [...e.currentTarget.querySelectorAll<HTMLElement>("[role^=menuitem]")];
    const i = items.indexOf(document.activeElement as HTMLElement);
    items[(i + (e.key === "ArrowDown" ? 1 : items.length - 1)) % items.length]?.focus();
  };
  return (
    <div className="relative" ref={ref}>
      {trigger({ open, toggle: () => setOpen(!open), ref: triggerRef })}
      {open && (
        <div role="menu" aria-label={label} onKeyDown={onKey} className="overlay absolute left-0 top-9 z-50 w-72 rounded-xl p-1">
          {children(close)}
        </div>
      )}
    </div>
  );
}

function MenuItem({ checked, disabled, reason, onClick, children }: { checked?: boolean; disabled?: boolean; reason?: string; onClick: () => void; children: ReactNode }) {
  return (
    <button
      type="button"
      role={checked === undefined ? "menuitem" : "menuitemradio"}
      aria-checked={checked}
      aria-disabled={disabled || undefined}
      title={disabled ? reason : undefined}
      onClick={disabled ? undefined : onClick}
      className="flex h-8 w-full items-center gap-2 rounded-md px-2 text-left text-[13px] text-fg hover:bg-white/[.08] aria-disabled:cursor-not-allowed aria-disabled:text-muted aria-disabled:hover:bg-transparent"
    >
      <span className="flex w-4 shrink-0 justify-center">{checked && <Check size={14} className="text-accent" />}</span>
      {children}
    </button>
  );
}

/** The grip: drag it (or the header) to move the panel, click it to pick a place; arrows move a floating panel. */
function PlaceMenu({ panel }: { panel: React.RefObject<HTMLElement | null> }) {
  const { mode, sideFits } = useDockLayout();
  return (
    <Menu
      label="Panel place"
      trigger={({ open, toggle, ref }) => (
        <button
          ref={ref}
          type="button"
          aria-label="Move panel"
          title={mode === "float" ? "Move panel: drag it, use the arrow keys, or click to dock it" : "Move panel: drag it, or click to pick a place"}
          aria-haspopup="menu"
          aria-expanded={open}
          onPointerDown={(e) => panel.current && startMove(e, panel.current, toggle)}
          onKeyDown={(e) => {
            if (e.key === "Enter" || e.key === " ") {
              e.preventDefault();
              toggle();
              return;
            }
            if (mode !== "float" || !e.key.startsWith("Arrow")) return;
            e.preventDefault();
            e.stopPropagation();
            const step = e.shiftKey ? 96 : 24;
            moveFloatBy(e.key === "ArrowLeft" ? -step : e.key === "ArrowRight" ? step : 0, e.key === "ArrowUp" ? -step : e.key === "ArrowDown" ? step : 0);
          }}
          className={`flex h-7 w-6 shrink-0 cursor-grab items-center justify-center rounded-md text-muted transition-colors duration-[120ms] hover:bg-white/[.08] hover:text-fg active:cursor-grabbing ${open ? "bg-white/[.08] text-fg" : ""}`}
        >
          <GripVertical size={15} />
        </button>
      )}
    >
      {(close) =>
        (["right", "left", "inspector", "float"] as DockMode[]).map((m) => (
          <MenuItem
            key={m}
            checked={mode === m}
            disabled={(m === "left" || m === "right") && !sideFits}
            reason={TOO_NARROW}
            onClick={() => {
              useDock.setState({ mode: m, open: true });
              close();
            }}
          >
            {DOCK_LABELS[m]}
          </MenuItem>
        ))
      }
    </Menu>
  );
}

function AgentMenu() {
  const agents = useAgent((s) => s.agents);
  const agent = useAgent((s) => s.agent);
  const busy = useAgent((s) => s.status !== "idle");
  const name = agents?.find((a) => a.id === agent)?.name ?? (agent === "codex" ? "Codex" : "Claude Code");
  return (
    <Menu
      label="Agent"
      trigger={({ open, toggle, ref }) => (
        <button
          ref={ref}
          type="button"
          aria-haspopup="menu"
          aria-expanded={open}
          title="Choose the agent"
          onClick={toggle}
          className={`flex h-7 min-w-0 items-center gap-1 rounded-md px-1.5 text-[13px] font-semibold text-fg hover:bg-white/[.08] ${open ? "bg-white/[.08]" : ""}`}
        >
          <span className="truncate">{name}</span>
          <ChevronDown size={14} className="shrink-0 text-muted" />
        </button>
      )}
    >
      {(close) => (
        <>
          {(agents ?? []).map((a) => (
            <MenuItem
              key={a.id}
              checked={a.id === agent}
              disabled={!a.path || busy || !READY.includes(a.id)}
              reason={!READY.includes(a.id) ? NOT_READY : !a.path ? `${a.name} is not installed` : "Wait for the answer, or stop it"}
              onClick={() => {
                chooseAgent(a.id);
                close();
              }}
            >
              <span className="flex-1">{a.name}</span>
              <span className="text-[12px] text-muted">{!READY.includes(a.id) ? "Coming next" : a.path ? (a.version ?? "Installed") : "Not installed"}</span>
            </MenuItem>
          ))}
          <div className="mx-1.5 my-1 h-px bg-white/[.08]" />
          <MenuItem
            onClick={() => {
              close();
              useEditor.setState({ connectOpen: true });
            }}
          >
            <Plug size={14} className="text-muted" />
            Connect an agent in a terminal…
          </MenuItem>
        </>
      )}
    </Menu>
  );
}

const STEP_ICON: Record<Step["status"], ReactNode> = {
  running: <Loader2 size={14} className="animate-spin text-accent" />,
  done: <Check size={14} className="text-ok" />,
  failed: <AlertCircle size={14} className="text-danger" />,
  stopped: <CircleStop size={14} className="text-muted" />,
};

function Steps({ steps, collapsed }: { steps: Step[]; collapsed: boolean }) {
  const [open, setOpen] = useState(!collapsed);
  useEffect(() => setOpen(!collapsed), [collapsed]);
  if (!open)
    return (
      <button type="button" onClick={() => setOpen(true)} className="-ml-1 flex items-center gap-1 self-start rounded-md px-1 py-0.5 text-[12px] text-muted hover:bg-white/[.06] hover:text-fg">
        <ChevronRight size={14} />
        {steps.length === 1 ? "1 step" : `${steps.length} steps`}
      </button>
    );
  return (
    <ol className="flex flex-col border-y border-white/[.06] py-1.5" aria-label="Steps">
      {steps.map((s) => (
        <li key={s.id} className="grid min-h-[26px] grid-cols-[18px_auto_minmax(0,1fr)] items-center gap-x-1.5 text-[12px]">
          {STEP_ICON[s.status]}
          <span className={s.status === "running" ? "text-fg" : "text-fg/80"}>{s.title}</span>
          <span className="tabular truncate text-right text-muted" title={s.detail ?? undefined}>
            {s.status === "stopped" ? "Stopped" : s.detail}
          </span>
        </li>
      ))}
    </ol>
  );
}

function Options({ item }: { item: Extract<ChatItem, { kind: "options" }> }) {
  const busy = useAgent((s) => s.status !== "idle");
  return (
    <div className="flex flex-col gap-1.5" role="group" aria-label={item.question ?? "Options"}>
      {item.question && <p className="leading-[20px] text-fg/90">{item.question}</p>}
      {item.options.map((o) => {
        const chosen = item.chosen === o.label;
        const off = item.chosen !== null || busy;
        return (
          <button
            key={o.label}
            type="button"
            aria-disabled={off || undefined}
            aria-pressed={chosen}
            title={off && !chosen ? (item.chosen ? "Already answered" : "Wait for the answer, or stop it") : undefined}
            onClick={off ? undefined : () => choose(item.id, o.label)}
            className={`flex flex-col items-start gap-0.5 rounded-[10px] px-3 py-2 text-left transition-colors duration-[120ms] ${
              chosen ? "bg-accent/[.18] shadow-[inset_0_0_0_1px_rgb(41_151_255/.6)]" : off ? "cursor-not-allowed bg-raised opacity-50" : "bg-raised hover:bg-[#35353a]"
            }`}
          >
            <span className="flex items-center gap-1.5 text-[13px] font-medium text-fg">
              {chosen && <Check size={13} className="text-accent" />}
              {o.label}
            </span>
            {o.detail && <span className="text-[12px] leading-[17px] text-muted">{o.detail}</span>}
          </button>
        );
      })}
    </div>
  );
}

function ChangeRow({ change, undone }: { change: RunChange; undone: boolean }) {
  // Recomputed when the project changes, never with the playhead while playing.
  const project = useEditor((s) => s.snap?.project);
  const ids = useMemo(() => {
    const all = new Set(project?.tracks.flatMap((t) => t.clips.map((c) => c.id)));
    return change.clipIds.filter((id) => all.has(id));
  }, [project, change]);
  const go = !undone && (ids.length > 0 || change.atUs !== null);
  const show = () => {
    const s = useEditor.getState();
    if (ids.length) s.select(ids);
    if (change.atUs !== null) s.seek(change.atUs);
  };
  return (
    <li>
      <button
        type="button"
        disabled={!go}
        onClick={show}
        title={go ? "Show it on the timeline" : undefined}
        className={`flex w-full items-start gap-2 rounded-md py-1 pl-[30px] pr-1.5 text-left text-[12px] leading-[17px] enabled:hover:bg-white/[.06] disabled:cursor-default ${undone ? "text-fg/45" : "text-fg/90"}`}
      >
        <span className="flex-1">{change.text}</span>
        {change.atUs !== null && <span className="tabular shrink-0 text-muted">{formatTime(change.atUs, false)}</span>}
      </button>
    </li>
  );
}

function RunCard({ item, index }: { item: Extract<ChatItem, { kind: "run" }>; index: number }) {
  // Undo is checked each render, so it turns off once later changes are on top of the run.
  useEditor((s) => `${s.snap?.sessionEpoch}:${s.snap?.revision}`);
  const locked = useEditor((s) => !!s.aiRun);
  const undo = item.snap && !item.undone ? undoAction(item.snap) : null;
  const valid = !!undo && (undo.valid?.() ?? true) && !locked;
  const label = item.undone ? `Undone: ${item.label}` : item.stopped ? `Stopped: ${item.label}` : item.label;
  return (
    <div className="flex flex-col gap-1.5 rounded-[10px] bg-raised p-3">
      <div className="flex items-center gap-2">
        {item.undone ? (
          <Undo2 size={16} className="shrink-0 text-muted" />
        ) : item.stopped ? (
          <CircleStop size={16} className="shrink-0 text-muted" />
        ) : (
          <CheckCircle2 size={16} className="shrink-0 text-ok" />
        )}
        <span className="min-w-0 flex-1 truncate text-[13px] font-semibold text-fg" title={label}>
          {label}
        </span>
        {undo && (
          <Button
            className="h-7 shrink-0 px-2.5 text-[12px]"
            disabled={!valid}
            disabledReason={locked ? "AI is editing. Wait for it, or stop it." : "Later changes are on top of this run. Undo them first."}
            title="Undo everything this run did"
            onClick={() => void undoStep(item.snap!).then((done) => done && markUndone(index))}
          >
            <Undo2 size={13} /> Undo
          </Button>
        )}
      </div>
      {item.changes && item.changes.length > 0 ? (
        <ul className="-mx-1.5 flex flex-col" aria-label="What changed">
          {item.changes.map((c, i) => (
            <ChangeRow key={i} change={c} undone={!!item.undone} />
          ))}
        </ul>
      ) : (
        !item.snap && <p className="text-[12px] text-muted">Nothing in the project changed.</p>
      )}
    </div>
  );
}

function ErrorNotice({ code, message, retryText }: { code: AgentErrorCode; message: string; retryText?: string }) {
  const agents = useAgent((s) => s.agents);
  const agent = useAgent((s) => s.agent);
  const name = agents?.find((a) => a.id === agent)?.name ?? "The agent";
  const other = agents?.find((a) => a.id !== agent && a.path && READY.includes(a.id));
  const retry = (
    <Button className="h-7 px-2.5 text-[12px]" onClick={() => void (retryText ? send(retryText) : send())}>
      Try again
    </Button>
  );
  const switchTo = other && (
    <Button variant="ghost" className="h-7 px-2.5 text-[12px]" title="Starts a new conversation; your message stays" onClick={() => chooseAgent(other.id)}>
      Ask {other.name} instead
    </Button>
  );
  const cmd = <code className="rounded bg-white/[.08] px-1">{COMMAND[agent]}</code>;
  const [icon, title, body, buttons]: [ReactNode, string, ReactNode, ReactNode] =
    code === "NOT_SIGNED_IN"
      ? [<AlertCircle size={16} className="text-danger" />, `${name} isn't signed in.`, <>Open a terminal, run {cmd} and sign in. Your message is still here.</>, <>{retry}{switchTo}</>]
      : code === "USAGE_LIMIT"
        ? [<AlertTriangle size={16} className="text-warn" />, `${name} usage limit reached.`, <>{message} The limit comes from your plan, not from Nuzky. Your message is still here.</>, switchTo]
        : code === "NOT_INSTALLED"
          ? [
              <AlertCircle size={16} className="text-danger" />,
              `${name} isn't installed.`,
              <>Install it and sign in from a terminal. Your message is still here.</>,
              <Button className="h-7 px-2.5 text-[12px]" onClick={() => void openUrl(INSTALL[agent])}>
                Install <ExternalLink size={12} />
              </Button>,
            ]
          : [<AlertCircle size={16} className="text-danger" />, `${name} stopped with an error.`, <Failure message={message} cmd={cmd} />, retry];
  return (
    <div role="alert" className="grid grid-cols-[20px_1fr] gap-2 rounded-[10px] bg-raised p-3 text-[12px] leading-[18px] text-fg/85">
      <span className="pt-px">{icon}</span>
      <div className="flex min-w-0 flex-col gap-0.5">
        <span className="text-[13px] font-semibold text-fg">{title}</span>
        <span className="flex flex-col items-start break-words">
          <span>{body}</span>
        </span>
        {buttons && <div className="mt-2 flex flex-wrap gap-1.5">{buttons}</div>}
      </div>
    </div>
  );
}

/** The first line of what went wrong and what to do; the agent's own lines behind Details. */
function Failure({ message, cmd }: { message: string; cmd: ReactNode }) {
  const [open, setOpen] = useState(false);
  const [first, ...rest] = message.trim().split("\n");
  return (
    <>
      <span className="line-clamp-2">{first}</span> Try again; if it keeps failing, run {cmd} in a terminal to see why.
      {rest.length > 0 &&
        (open ? (
          <pre className="mt-1.5 max-h-40 overflow-auto whitespace-pre-wrap rounded-md bg-white/[.06] px-2 py-1.5 text-[11px] leading-[16px] text-muted">{rest.join("\n")}</pre>
        ) : (
          <button type="button" onClick={() => setOpen(true)} className="mt-1 self-start text-[12px] text-accent hover:underline">
            Details
          </button>
        ))}
    </>
  );
}

const STARTERS: [ReactNode, string][] = [
  [<Scissors size={14} />, "Cut retakes and long pauses"],
  [<Captions size={14} />, "Add captions"],
  [<Download size={14} />, "Make a reel and export it"],
];

function Empty() {
  const agents = useAgent((s) => s.agents);
  const error = useAgent((s) => s.agentsError);
  const blocked = useEditor((s) => !!s.aiRun);
  if (error) return <div className="mt-auto"><ErrorNotice code="AGENT_FAILED" message={error} /></div>;
  if (agents === null) return <p className="mt-auto px-1 text-[12px] text-muted">Looking for Claude Code and Codex…</p>;
  if (!agents.some((a) => a.path && READY.includes(a.id)))
    return (
      <div className="my-auto flex flex-col px-1">
        <h3 className="text-[15px] font-semibold text-fg">No AI agent found</h3>
        <p className="mb-3 mt-1.5 text-[12px] leading-[18px] text-muted">Nuzky runs the AI you already use. Install one, sign in from your terminal, then check again.</p>
        {agents.filter((a) => READY.includes(a.id)).map((a) => (
          <div key={a.id} className="flex h-9 items-center justify-between border-t border-white/[.07] text-[13px]">
            {a.name}
            <Button variant="ghost" className="h-7 px-2 text-[12px] text-accent" onClick={() => void openUrl(INSTALL[a.id])}>
              Install <ExternalLink size={12} />
            </Button>
          </div>
        ))}
        <Button className="mt-3 h-7 self-start px-2.5 text-[12px]" onClick={() => void loadAgents()}>
          <RefreshCw size={13} /> Check again
        </Button>
      </div>
    );
  return (
    <div className="mt-auto flex flex-col gap-0.5">
      <p className="mb-2 px-2 text-[12px] text-muted">Edits show up live. One Undo takes back a whole run.</p>
      {STARTERS.map(([icon, text]) => (
        <button
          key={text}
          type="button"
          aria-disabled={blocked || undefined}
          title={blocked ? "Another agent is editing" : "Put this in the field, to send or change"}
          onClick={
            blocked
              ? undefined
              : () => {
                  useAgent.setState({ draft: text });
                  document.querySelector<HTMLTextAreaElement>("aside[aria-label=AI] textarea")?.focus();
                }
          }
          className="flex h-8 items-center gap-2.5 rounded-md px-2 text-left text-[13px] text-fg hover:bg-white/[.08] aria-disabled:cursor-not-allowed aria-disabled:opacity-40 aria-disabled:hover:bg-transparent [&>svg]:text-muted"
        >
          {icon}
          {text}
        </button>
      ))}
    </div>
  );
}

function Item({ item, index, collapsed }: { item: ChatItem; index: number; collapsed: boolean }) {
  switch (item.kind) {
    case "user":
      return (
        <div className="flex max-w-[88%] flex-col items-end gap-1 self-end">
          <div className="whitespace-pre-wrap break-words rounded-[14px] rounded-br-[4px] bg-raised px-3 py-2 leading-[19px] text-fg">{item.text}</div>
          {item.context && <div className="tabular text-[11px] text-muted">{item.context}</div>}
        </div>
      );
    case "agent":
      return (
        <div className="break-words leading-[20px] text-fg/90">
          <Markdown text={item.text} />
        </div>
      );
    case "steps":
      return <Steps steps={item.steps} collapsed={collapsed} />;
    case "options":
      return <Options item={item} />;
    case "run":
      return <RunCard item={item} index={index} />;
    case "stopped":
      return (
        <div className="flex items-center gap-2 rounded-[10px] bg-raised p-3 text-[12px]">
          <CircleStop size={16} className="text-muted" />
          <span className="text-[13px] font-semibold text-fg">Stopped</span>
          <span className="text-muted">Nothing changed</span>
        </div>
      );
    case "error":
      return <ErrorNotice code={item.code} message={item.message} retryText={item.retry} />;
  }
}

function Conversation() {
  const items = useAgent((s) => s.items);
  const working = useAgent((s) => s.status !== "idle");
  const scroller = useRef<HTMLDivElement>(null);
  const pinned = useRef(true);
  // Follows new content while the user is at the bottom; scrolling up to read stops that.
  useLayoutEffect(() => {
    const el = scroller.current;
    if (el && pinned.current) el.scrollTop = el.scrollHeight;
  }, [items]);
  const lastUser = items.map((i) => i.kind).lastIndexOf("user");
  return (
    <div
      ref={scroller}
      onScroll={(e) => {
        const el = e.currentTarget;
        pinned.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
      }}
      className="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto px-3.5 pb-2 pt-3.5 text-[13px]"
    >
      {items.length === 0 ? <Empty /> : <div className="mt-auto" />}
      {items.map((item, i) => (
        // Steps fold away once the agent has answered after them.
        <Item key={i} item={item} index={i} collapsed={item.kind === "steps" && (i < lastUser || (!working && items.slice(i + 1).some((x) => x.kind === "agent")))} />
      ))}
    </div>
  );
}

function Chip({ icon, children, onRemove, label }: { icon: ReactNode; children: ReactNode; onRemove: () => void; label: string }) {
  return (
    <span className="flex h-[22px] items-center gap-1.5 whitespace-nowrap rounded-md bg-white/[.08] pl-1.5 text-[12px] text-fg/90 [&>svg]:text-muted">
      {icon}
      {children}
      <button
        type="button"
        aria-label={`Don't send ${label}`}
        title={`Don't send ${label}`}
        onClick={(e) => {
          e.stopPropagation();
          onRemove();
        }}
        className="flex h-[22px] w-5 items-center justify-center rounded-r-md text-muted hover:bg-white/[.08] hover:text-fg"
      >
        <X size={11} />
      </button>
    </span>
  );
}

function Elapsed() {
  // The start lives in the store, so moving the panel during a run does not restart the count.
  const start = useAgent((s) => s.startedAt) ?? Date.now();
  const [, tick] = useState(0);
  useEffect(() => {
    const id = window.setInterval(() => tick((n) => n + 1), 1000);
    return () => window.clearInterval(id);
  }, []);
  const s = Math.floor((Date.now() - start) / 1000);
  return <>{`${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`}</>;
}

function Composer() {
  const draft = useAgent((s) => s.draft);
  const status = useAgent((s) => s.status);
  const frame = useAgent((s) => s.frame);
  const dropSelection = useAgent((s) => s.dropSelection);
  const dropPlayhead = useAgent((s) => s.dropPlayhead);
  const agents = useAgent((s) => s.agents);
  const agent = useAgent((s) => s.agent);
  const hasItems = useAgent((s) => s.items.length > 0);
  const selected = useEditor((s) => s.selection.length);
  // From the selection and the project only, so playing does not recompute it every frame.
  const selection = useEditor((s) => s.selection);
  const project = useEditor((s) => s.snap?.project);
  const range = useMemo(() => {
    const chosen = new Set(selection);
    const clips = project?.tracks.flatMap((t) => t.clips).filter((c) => chosen.has(c.id)) ?? [];
    if (!clips.length) return null;
    const start = clips.reduce((m, c) => Math.min(m, c.startUs), Infinity);
    const end = clips.reduce((m, c) => Math.max(m, c.startUs + c.durationUs), 0);
    return `${formatTime(start, false)}–${formatTime(end, false)}`;
  }, [selection, project]);
  // Whole seconds, so the chip does not redraw with every frame while playing.
  const playhead = useEditor((s) => formatTime(s.timeUs, false));
  const otherRun = useEditor((s) => (s.aiRun && status === "idle" ? s.aiRun : null));
  const field = useRef<HTMLTextAreaElement>(null);
  const name = agents?.find((a) => a.id === agent)?.name ?? "the agent";
  const working = status !== "idle";
  const blocked = sendBlocked();

  useLayoutEffect(() => {
    const el = field.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, 160)}px`;
  }, [draft]);
  // The field takes focus when the panel opens, so typing can start at once.
  useEffect(() => field.current?.focus(), []);

  // Enter that cannot send says why for a moment, instead of doing nothing.
  const [why, setWhy] = useState<string | null>(null);
  useEffect(() => {
    if (!why) return;
    const id = window.setTimeout(() => setWhy(null), 3000);
    return () => window.clearTimeout(id);
  }, [why]);
  // A new state makes an old reason wrong.
  useEffect(() => setWhy(null), [status]);
  // Fresh from the store: Enter can come before React has drawn the latest text.
  const submit = () => {
    if (!useAgent.getState().draft.trim()) return;
    const reason = sendBlocked();
    if (reason) setWhy(reason);
    else void send();
  };

  return (
    <div className="shrink-0 px-2.5 pb-2.5">
      {otherRun && (
        <div role="status" className="mb-2 grid grid-cols-[20px_1fr] gap-2 rounded-[10px] bg-raised p-3 text-[12px] leading-[18px] text-fg/85">
          <Info size={16} className="text-accent" />
          <div>
            <div className="text-[13px] font-semibold text-fg">Another agent is editing.</div>
            {otherRun}. Wait until it finishes, or stop it with Stop and edit in the top bar.
          </div>
        </div>
      )}
      <div onClick={() => field.current?.focus()} className="rounded-xl border border-white/[.08] bg-white/[.055] px-2.5 pb-1.5 pt-2 transition-colors duration-[120ms] has-[textarea:focus]:border-accent">
        {!working && ((selected > 0 && !dropSelection) || !dropPlayhead) && (
          <div className="mb-2 flex flex-wrap gap-1.5">
            {selected > 0 && !dropSelection && (
              <Chip icon={<Film size={12} />} label="the selected clips" onRemove={() => useAgent.setState({ dropSelection: true })}>
                {selected === 1 ? "1 clip" : `${selected} clips`}
                {range && <span className="tabular text-muted">· {range}</span>}
              </Chip>
            )}
            {!dropPlayhead && (
              <Chip icon={<Crosshair size={12} />} label="the playhead" onRemove={() => useAgent.setState({ dropPlayhead: true })}>
                <span className="tabular">Playhead {playhead}</span>
              </Chip>
            )}
          </div>
        )}
        <textarea
          ref={field}
          rows={2}
          value={draft}
          aria-label={`Message to ${name}`}
          placeholder={working ? `${name} is working…` : hasItems ? "Ask for changes…" : `Ask ${name} to edit…`}
          onChange={(e) => useAgent.setState({ draft: e.target.value })}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
              e.preventDefault();
              submit();
            }
          }}
          className="block max-h-40 w-full resize-none bg-transparent text-[13px] leading-[19px] text-fg outline-none placeholder:text-muted"
        />
        <div className="mt-1 flex items-center gap-2">
          <IconButton
            label={frame ? "Don't send the frame at the playhead" : "Send the frame at the playhead too"}
            active={frame}
            disabled={working}
            onClick={() => useAgent.setState({ frame: !frame })}
            className="h-7 w-7"
          >
            <ImagePlus size={15} />
          </IconButton>
          <span className="flex-1" />
          {!working && why && (
            <span className="min-w-0 truncate text-[12px] text-fg/85" role="status">
              {why}
            </span>
          )}
          {working ? (
            <>
              <span className="tabular flex min-w-0 items-center gap-1.5 text-[12px] text-muted" role="status">
                <Loader2 size={13} className="shrink-0 animate-spin text-accent" />
                {why ? (
                  <span className="truncate text-fg/85">{why}</span>
                ) : status === "stopping" ? (
                  "Stopping…"
                ) : (
                  <>
                    Working · <Elapsed />
                  </>
                )}
              </span>
              <Button className="h-7 px-2.5 text-[12px]" disabled={status === "stopping"} title="Stop the agent. What it did so far stays, and Undo takes it back." onClick={() => void stop()}>
                <CircleStop size={13} /> Stop
              </Button>
            </>
          ) : (
            <button
              type="button"
              aria-label="Send"
              title={blocked ?? (draft.trim() ? "Send (Enter)" : "Type a message first")}
              aria-disabled={!!blocked || !draft.trim() || undefined}
              onClick={submit}
              className="flex h-7 w-7 items-center justify-center rounded-full bg-accent-strong text-white transition-colors duration-[120ms] hover:bg-[#0064c8] aria-disabled:cursor-not-allowed aria-disabled:bg-white/[.08] aria-disabled:text-white/40"
            >
              <ArrowUp size={16} />
            </button>
          )}
        </div>
      </div>
    </div>
  );
}

/** The AI chat: header, conversation and the field to write in. AiDock places it. */
export function AiPanel({ panelRef, className = "", style }: { panelRef: React.RefObject<HTMLElement | null>; className?: string; style?: React.CSSProperties }) {
  const busy = useAgent((s) => s.status !== "idle");
  const empty = useAgent((s) => s.items.length === 0);
  const { mode } = useDockLayout();
  useEffect(() => {
    if (useAgent.getState().agents === null) void loadAgents();
  }, []);
  const move = (e: React.PointerEvent) => panelRef.current && startMove(e, panelRef.current);
  return (
    <aside
      ref={panelRef as React.RefObject<HTMLElement>}
      aria-label="AI"
      data-dock-slot={mode === "inspector" ? "inspector" : undefined}
      className={`pane flex flex-col overflow-hidden ${className}`}
      style={style}
    >
      <header
        // The header's empty space moves the panel too; its buttons keep their clicks.
        onPointerDown={(e) => e.target === e.currentTarget && move(e)}
        className="flex h-12 shrink-0 items-center gap-1 border-b border-white/[.07] pl-1.5 pr-2"
      >
        <PlaceMenu panel={panelRef} />
        <AgentMenu />
        <span className="flex-1 self-stretch" onPointerDown={move} />
        <IconButton
          label={busy ? "New chat: wait for the answer, or stop it" : empty ? "New chat: there is nothing to clear yet" : "New chat"}
          disabled={busy || empty}
          onClick={newChat}
          className="h-7 w-7"
        >
          <SquarePen size={15} />
        </IconButton>
        <IconButton label="Close AI (Ctrl+J)" onClick={() => useDock.setState({ open: false })} className="h-7 w-7">
          <X size={16} />
        </IconButton>
      </header>
      <Conversation />
      <Composer />
    </aside>
  );
}
