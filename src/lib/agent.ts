import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { create } from "zustand";
import { errorText, plainError } from "./api";
import { formatTime } from "./time";
import { useEditor } from "./store";
import type { Snapshot } from "./types";

/** An agent the panel can run: the user's own installed Claude Code or Codex. */
export type AgentId = "claude" | "codex";
export interface AgentInfo {
  id: AgentId;
  name: string;
  /** Where the executable was found, or null when it is not installed. */
  path: string | null;
  version: string | null;
}

/** What went wrong with a turn, so the panel can say what to do next. */
export type AgentErrorCode = "NOT_INSTALLED" | "NOT_SIGNED_IN" | "USAGE_LIMIT" | "AGENT_FAILED";

/** Events from the backend for one chat, in order. */
export type AgentEvent =
  | { chat: string; type: "text"; text: string }
  | { chat: string; type: "tool"; id: string; name: string; status: "running" | "done" | "failed"; input?: unknown }
  | { chat: string; type: "done"; stopped: boolean }
  | { chat: string; type: "error"; code: AgentErrorCode; message: string };

/** One line of what a run changed, from comparing the project before and after it. */
export interface RunChange {
  text: string;
  /** Clips the line is about, to select them. */
  clipIds: string[];
  /** Where on the timeline to look. */
  atUs: number | null;
}

export interface Step {
  id: string;
  title: string;
  status: "running" | "done" | "failed" | "stopped";
  detail: string | null;
}

export interface Choice {
  label: string;
  detail: string | null;
}

export type ChatItem =
  | { kind: "user"; text: string; context: string | null }
  | { kind: "agent"; text: string }
  | { kind: "steps"; steps: Step[] }
  | { kind: "options"; id: string; question: string | null; options: Choice[]; chosen: string | null }
  | { kind: "run"; label: string; snap: Snapshot | null; stopped: boolean; changes: RunChange[] | null; undone?: boolean }
  | { kind: "stopped" }
  | { kind: "error"; code: AgentErrorCode; message: string };

export interface PromptContext {
  /** Selected clips at send time, or null when the user removed the chip or nothing is selected. */
  selection: string[] | null;
  playheadUs: number | null;
  frame: boolean;
}

interface AgentState {
  agents: AgentInfo[] | null;
  agentsError: string | null;
  agent: AgentId;
  chat: string | null;
  items: ChatItem[];
  status: "idle" | "working" | "stopping";
  /** When the current answer started, in ms; it survives moving the panel. */
  startedAt: number | null;
  draft: string;
  /** Context chips the user removed from the next message. */
  dropSelection: boolean;
  dropPlayhead: boolean;
  frame: boolean;
}

const AGENT_KEY = "capopen.aiAgent";

/** Agents the panel can run so far; Codex there comes next, in a terminal it works today. */
export const READY: AgentId[] = ["claude"];
export const NOT_READY = "Codex in the AI panel comes next. Until then, connect it in a terminal.";
const usable = (a: AgentInfo) => !!a.path && READY.includes(a.id);

export const useAgent = create<AgentState>(() => ({
  agents: null,
  agentsError: null,
  agent: READY.includes(localStorage.getItem(AGENT_KEY) as AgentId) ? (localStorage.getItem(AGENT_KEY) as AgentId) : "claude",
  chat: null,
  items: [],
  status: "idle",
  startedAt: null,
  draft: "",
  dropSelection: false,
  dropPlayhead: false,
  frame: false,
}));

export const agentApi = {
  list: () => invoke<AgentInfo[]>("agent_list"),
  send: (agent: AgentId, chat: string, text: string, context: PromptContext) => invoke<void>("agent_send", { agent, chat, text, context }),
  stop: (chat: string) => invoke<void>("agent_stop", { chat }),
};

/** Tool names as the agent reports them: `mcp__capopen__get_state` (Claude) or `capopen.get_state` (Codex). */
const toolName = (raw: string) => raw.replace(/^mcp__capopen__/, "").replace(/^capopen[./]/, "");

/** Bookkeeping calls the user does not need to see as steps. */
const HIDDEN = new Set(["begin_run", "end_run", "job"]);
/** The tool an agent offers choices with; the panel shows them as buttons, not as a step. */
const OFFER = "suggest_options";

const TITLES: Record<string, string> = {
  get_state: "Read the project",
  get_transcript: "Read the transcript",
  transcribe: "Transcribing",
  apply_edits: "Editing",
  edit_transcript: "Cutting by the transcript",
  correct_words: "Correcting words",
  apply_zooms: "Adding zooms",
  build_captions: "Adding captions",
  inspect_frames: "Checking frames",
  import_media: "Importing media",
  export_video: "Exporting",
  undo_run: "Undoing its changes",
};

const ANALYSES: Record<string, string> = {
  silences: "Finding pauses",
  loudness: "Measuring loudness",
  scenes: "Finding scene changes",
  fillers: "Finding filler words",
  retakes: "Finding retakes",
  emphasis: "Finding sentences to zoom",
};

const obj = (v: unknown): Record<string, unknown> => (v && typeof v === "object" ? (v as Record<string, unknown>) : {});

/** A short step title for a tool call, from what it was asked; tools from elsewhere keep their own name. */
export function stepTitle(raw: string, input?: unknown): string {
  const name = toolName(raw);
  if (name === "analyze") return ANALYSES[String(obj(input).kind)] ?? "Analysing the sound";
  return TITLES[name] ?? name;
}

/** The detail worth showing next to a step: which frames, which file. */
export function stepDetail(raw: string, input?: unknown): string | null {
  const args = obj(input);
  const name = toolName(raw);
  if (name === "inspect_frames" && Array.isArray(args.times)) return args.times.slice(0, 4).map((t) => formatTime(Number(t), false)).join(" · ") + (args.times.length > 4 ? " …" : "");
  if (name === "export_video" && typeof args.path === "string") return args.path.split(/[\\/]/).pop() ?? null;
  if (name === "apply_edits" && Array.isArray(args.edits)) return args.edits.length === 1 ? "1 change" : `${args.edits.length} changes`;
  if (name === "import_media" && Array.isArray(args.paths)) return args.paths.length === 1 ? String(args.paths[0]).split(/[\\/]/).pop()! : `${args.paths.length} files`;
  return null;
}

function offered(input: unknown): Omit<Extract<ChatItem, { kind: "options" }>, "id" | "kind" | "chosen"> | null {
  const args = obj(input);
  if (!Array.isArray(args.options)) return null;
  const options = args.options
    .map((o) => (typeof o === "string" ? { label: o, detail: null } : { label: String(obj(o).label ?? ""), detail: typeof obj(o).detail === "string" ? (obj(o).detail as string) : null }))
    .filter((o) => o.label.trim());
  return options.length ? { question: typeof args.question === "string" ? args.question : null, options } : null;
}

export async function loadAgents() {
  try {
    const agents = await agentApi.list();
    const s = useAgent.getState();
    // Keep the chosen agent when it is installed; otherwise pick one that is.
    const chosen = agents.find((a) => a.id === s.agent && usable(a)) ?? agents.find(usable);
    useAgent.setState({ agents, agentsError: null, agent: chosen?.id ?? s.agent });
  } catch (e) {
    useAgent.setState({ agentsError: plainError(errorText(e)) });
  }
}

export function chooseAgent(agent: AgentId) {
  const s = useAgent.getState();
  if (s.agent === agent || s.status !== "idle" || !READY.includes(agent)) return;
  localStorage.setItem(AGENT_KEY, agent);
  // Another agent does not know this conversation, so it starts a new one; the draft stays.
  useAgent.setState({ agent, chat: null, items: [] });
}

export function newChat() {
  if (useAgent.getState().status !== "idle") return;
  useAgent.setState({ chat: null, items: [] });
}

/** What the next message would carry, as the chips show it. */
export function promptContext(): PromptContext {
  const { selection, timeUs } = useEditor.getState();
  const { dropSelection, dropPlayhead, frame } = useAgent.getState();
  return { selection: dropSelection || selection.length === 0 ? null : [...selection], playheadUs: dropPlayhead ? null : Math.round(timeUs), frame };
}

export function describeContext(c: PromptContext): string | null {
  const parts: string[] = [];
  if (c.selection) parts.push(c.selection.length === 1 ? "1 clip" : `${c.selection.length} clips`);
  if (c.playheadUs !== null) parts.push(`Playhead ${formatTime(c.playheadUs, false)}`);
  if (c.frame) parts.push("Frame");
  return parts.length ? parts.join(" · ") : null;
}

/** Why a message cannot be sent right now, or null. */
export function sendBlocked(): string | null {
  const s = useAgent.getState();
  const info = s.agents?.find((a) => a.id === s.agent);
  if (!READY.includes(s.agent)) return NOT_READY;
  if (s.agents && !info?.path) return `${info?.name ?? "The agent"} is not installed`;
  if (s.status !== "idle") return "Wait for the answer, or stop it";
  if (useEditor.getState().aiRun) return "Another agent is editing. Wait for it, or stop it in the top bar";
  return null;
}

export async function send(text = useAgent.getState().draft.trim()) {
  const s = useAgent.getState();
  if (!text || sendBlocked()) return;
  const context = promptContext();
  const fromDraft = text === s.draft.trim();
  // The chat is named here, before sending, so even an agent that fails at once is heard.
  const chat = s.chat ?? crypto.randomUUID();
  useAgent.setState({
    chat,
    // Earlier problems are over once a new message goes; their Try again would send this one.
    items: [...s.items.filter((i) => i.kind !== "error"), { kind: "user", text, context: describeContext(context) }],
    status: "working",
    startedAt: Date.now(),
    draft: fromDraft ? "" : s.draft,
    dropSelection: false,
    dropPlayhead: false,
    frame: false,
  });
  try {
    await agentApi.send(s.agent, chat, text, context);
  } catch (e) {
    const text = errorText(e);
    fail(text.startsWith("NOT_INSTALLED") ? "NOT_INSTALLED" : "AGENT_FAILED", plainError(text));
  }
}

/** The run card's own Undo ran: it says so instead of offering Undo again. */
export function markUndone(index: number) {
  useAgent.setState((s) => ({ items: s.items.map((it, i) => (i === index && it.kind === "run" ? { ...it, undone: true } : it)) }));
}

/** Answers the agent's question with the option picked. */
export function choose(id: string, label: string) {
  if (sendBlocked()) return;
  useAgent.setState((s) => ({ items: s.items.map((it) => (it.kind === "options" && it.id === id ? { ...it, chosen: label } : it)) }));
  void send(label);
}

export async function stop() {
  const { chat, status } = useAgent.getState();
  if (!chat || status !== "working") return;
  useAgent.setState({ status: "stopping" });
  try {
    await agentApi.stop(chat);
  } catch (e) {
    useAgent.setState({ status: "working" });
    useEditor.getState().toast({ kind: "error", text: plainError(errorText(e)) });
  }
}

/**
 * Ends the turn with an error. A turn that failed before the agent answered gives its message back
 * to the field, so it is never lost; the user message leaves the conversation then.
 */
function fail(code: AgentErrorCode, message: string) {
  const s = useAgent.getState();
  let items = s.items;
  let draft = s.draft;
  const last = items[items.length - 1];
  if (last?.kind === "user") {
    items = items.slice(0, -1);
    draft = draft ? `${last.text}\n${draft}` : last.text;
  }
  useAgent.setState({ items: [...items, { kind: "error", code, message }], draft, status: "idle" });
}

function onEvent(e: AgentEvent) {
  const s = useAgent.getState();
  if (e.chat !== s.chat) return;
  const items = [...s.items];
  const last = items[items.length - 1];
  if (e.type === "text") {
    if (last?.kind === "agent") items[items.length - 1] = { kind: "agent", text: last.text + e.text };
    else items.push({ kind: "agent", text: e.text });
    useAgent.setState({ items });
  } else if (e.type === "tool") {
    const name = toolName(e.name);
    if (HIDDEN.has(name)) return;
    if (name === OFFER) {
      const offer = offered(e.input);
      if (offer && !items.some((it) => it.kind === "options" && it.id === e.id)) useAgent.setState({ items: [...items, { kind: "options", id: e.id, chosen: null, ...offer }] });
      return;
    }
    const step: Step = { id: e.id, title: stepTitle(e.name, e.input), status: e.status, detail: stepDetail(e.name, e.input) };
    if (last?.kind === "steps") {
      const known = last.steps.find((x) => x.id === e.id);
      // Updates may come without the input; keep what the first one said.
      const steps = known ? last.steps.map((x) => (x.id === e.id ? { ...x, status: e.status, detail: step.detail ?? x.detail } : x)) : [...last.steps, step];
      items[items.length - 1] = { kind: "steps", steps };
    } else items.push({ kind: "steps", steps: [step] });
    useAgent.setState({ items });
  } else if (e.type === "error") {
    fail(e.code, e.message);
  } else {
    // A stopped turn marks the step it was on; one that changed nothing says so.
    const marked = items.map((it) => (it.kind === "steps" ? { ...it, steps: it.steps.map((x) => (x.status === "running" ? { ...x, status: e.stopped ? ("stopped" as const) : ("done" as const) } : x)) } : it));
    const turnStart = marked.map((it) => it.kind).lastIndexOf("user") + 1;
    const turn = marked.slice(turnStart);
    // What the turn changed comes last, after the agent's closing words.
    const runs = turn.filter((it) => it.kind === "run");
    const ordered = [...marked.slice(0, turnStart), ...turn.filter((it) => it.kind !== "run"), ...runs];
    useAgent.setState({ items: e.stopped && !runs.length ? [...ordered, { kind: "stopped" }] : ordered, status: "idle" });
  }
}

/** What the run ending next changed; it arrives just before the run ends. */
let pendingChanges: RunChange[] | null = null;

/**
 * The run the panel's agent opened has ended. Returns true when the panel shows it (with Undo),
 * so the editor does not also toast it; a run from an agent in a terminal is not the panel's.
 */
export function panelRunEnded(label: string, snap: Snapshot | null): boolean {
  const s = useAgent.getState();
  const changes = pendingChanges;
  pendingChanges = null;
  if (s.status === "idle") return false;
  useAgent.setState({ items: [...s.items, { kind: "run", label, snap, stopped: s.status === "stopping", changes }] });
  return true;
}

// Another project starts a new conversation; the backend stops an agent still working on the old one.
useEditor.subscribe((s, prev) => {
  if (prev.snap && s.snap?.sessionEpoch !== prev.snap.sessionEpoch) useAgent.setState({ chat: null, items: [], status: "idle" });
});

let listening = false;
export function listenAgentEvents() {
  if (listening) return;
  listening = true;
  void listen<AgentEvent>("agent", (e) => onEvent(e.payload));
  void listen<RunChange[]>("run-summary", (e) => (pendingChanges = e.payload));
}
