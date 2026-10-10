import { useEffect, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { AlertCircle, Check, ChevronDown, ChevronRight, FileText, History, MoreHorizontal, Pencil, Plus, Sparkles, Trash2 } from "lucide-react";
import { useEditor } from "../../lib/store";
import { changeStyle, loadStyle, useStyle } from "../../lib/style";
import { when } from "../../lib/time";
import type { StyleConfidence, StyleFrozen, StyleRule, StyleSource, StyleView, Suggestion } from "../../lib/types";
import { Button, IconButton, Menu, type MenuEntry } from "../ui";
import { LearnDialog } from "./LearnDialog";

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

/** A moment as learning quotes it: minutes, seconds and tenths. */
function clock(us: number) {
  const tenths = Math.floor(Math.max(0, us) / 100_000);
  return `${Math.floor(tenths / 600)}:${String(Math.floor(tenths / 10) % 60).padStart(2, "0")}.${tenths % 10}`;
}

const LEVEL = { high: ["High", 3], medium: ["Medium", 2], low: ["Low", 1] } as const;

/** Three bars, as many lit as the confidence is high. */
function Bars({ lit }: { lit: number }) {
  return (
    <span aria-hidden className="flex h-3 shrink-0 items-end gap-[2px]">
      {[5, 8, 11].map((h, i) => (
        <span key={h} className={`w-[3px] rounded-[1px] ${i < lit ? "bg-fg/85" : "bg-white/20"}`} style={{ height: h }} />
      ))}
    </span>
  );
}

function Confidence({ c }: { c: StyleConfidence }) {
  const [label, lit] = LEVEL[c.level];
  return (
    <span className="flex items-center gap-1.5 text-[12px] text-muted" title={`${label} confidence: seen in ${c.videos} of the ${plural(c.of, "video")} learned from`}>
      <Bars lit={lit} />
      <span className="text-fg/85">{label}</span>
      <span aria-hidden>·</span>
      <span className="tabular">
        in {c.videos} of {plural(c.of, "video")} · {plural(c.moments, "moment")}
      </span>
    </span>
  );
}

const MOMENTS_SHOWN = 4;

function SuggestionCard({ s, busy }: { s: Suggestion; busy: boolean }) {
  const [open, setOpen] = useState(false);
  const [all, setAll] = useState(false);
  const shown = all ? s.moments : s.moments.slice(0, MOMENTS_SHOWN);
  return (
    <section role="group" aria-label={`Suggestion: ${s.title}`} className="rounded-xl bg-white/[.055] p-4 shadow-[inset_0_0_0_1px_rgb(255_255_255/.06)]">
      <div className="flex items-start gap-3">
        <div className="flex min-w-0 flex-1 flex-col gap-1">
          <h3 className="flex items-baseline gap-2 text-[13px] font-semibold text-fg">
            {s.title}
            <span className="text-[12px] font-normal text-muted">{s.update ? "Change" : "New rule"}</span>
          </h3>
          <p className="text-[13px] text-fg">{s.summary}</p>
          {s.update && s.changes.length > 0 && (
            <p className="text-[12px] text-muted">
              {s.changes.map((c, i) => (
                <span key={c.name}>
                  {i > 0 && " · "}
                  {c.name}: {c.from ?? "none"} → <span className="text-fg">{c.to ?? "none"}</span>
                </span>
              ))}
            </p>
          )}
          <div className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-1">
            <Confidence c={s.confidence} />
            <button type="button" aria-expanded={open} className="flex items-center gap-0.5 text-[12px] text-accent hover:underline" onClick={() => setOpen(!open)}>
              {open ? "Hide moments" : "Show moments"}
              {open ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
            </button>
          </div>
        </div>
        <Button pill disabled={busy} disabledReason="Wait for the style to change" onClick={() => void changeStyle({ type: "reject", title: s.title }, `Learning leaves ${s.title} out`)}>
          Reject
        </Button>
        <Button pill disabled={busy} disabledReason="Wait for the style to change" onClick={() => void changeStyle({ type: "accept", titles: [s.title] })}>
          <Check size={14} /> Accept
        </Button>
      </div>
      {open && (
        <div className="mt-3 border-t border-white/[.08] pt-3">
          <ul className="grid grid-cols-[minmax(0,max-content)_auto_minmax(0,1fr)] gap-x-4 gap-y-1.5 text-[12px]" aria-label={`Moments of ${s.title}`}>
            {shown.map((m, i) => (
              <li key={i} className="contents">
                <span className="truncate text-fg/85">{m.video}</span>
                <span className="tabular text-muted">{clock(m.timeUs)}</span>
                <span className="text-fg/85">{m.line}</span>
              </li>
            ))}
          </ul>
          {s.moments.length > MOMENTS_SHOWN && (
            <button type="button" className="mt-2 text-[12px] text-accent hover:underline" onClick={() => setAll(!all)}>
              {all ? "Show fewer" : `Show all ${s.moments.length}`}
            </button>
          )}
        </div>
      )}
    </section>
  );
}

function SectionTitle({ title, count, children }: { title: string; count?: number; children?: ReactNode }) {
  return (
    <div className="mb-2 flex h-8 items-center gap-2">
      <h2 className="text-[13px] font-semibold text-fg">{title}</h2>
      {count !== undefined && <span className="tabular text-[12px] text-muted">{count}</span>}
      <div className="flex-1" />
      {children}
    </div>
  );
}

/** What changed since the style was shown before, by "own:<text>" and "rule:<title>", for a moment. */
function useChanged(view: StyleView | null): Set<string> {
  const before = useRef<StyleView | null>(null);
  const [changed, setChanged] = useState<Set<string>>(new Set());
  useEffect(() => {
    const old = before.current;
    before.current = view;
    if (!old || !view || old.version === view.version) return;
    const keys = new Set<string>();
    for (const rule of view.own) if (!old.own.includes(rule)) keys.add(`own:${rule}`);
    for (const rule of view.rules) {
      const was = old.rules.find((r) => r.title === rule.title);
      if (!was || was.summary !== rule.summary || was.byYou !== rule.byYou) keys.add(`rule:${rule.title}`);
    }
    setChanged(keys);
    const id = window.setTimeout(() => setChanged(new Set()), 2400);
    return () => window.clearTimeout(id);
  }, [view]);
  return changed;
}

/** Puts back the version before a change the toast names. */
const undoTo = (version: number) => ({ label: "Undo", run: () => void changeStyle({ type: "restore", index: version }) });

function OwnRules({ view, changed }: { view: StyleView; changed: Set<string> }) {
  const [draft, setDraft] = useState("");
  const [editing, setEditing] = useState<number | null>(null);
  const [edit, setEdit] = useState("");
  const editingWas = useRef<string | null>(null);
  const add = async () => {
    const text = draft.trim();
    if (text && (await changeStyle({ type: "setOwn", index: null, text, was: null }))) setDraft("");
  };
  // Each change names the rule as it was, so a rule the AI moved meanwhile is never changed instead.
  const save = async (i: number) => {
    const text = edit.trim();
    if (!text) return;
    if (await changeStyle({ type: "setOwn", index: i, text, was: editingWas.current })) setEditing(null);
  };
  const remove = (i: number) =>
    void changeStyle({ type: "setOwn", index: i, text: "", was: view.own[i] }).then((done) => done && toastUndo("Removed your rule", view.version));
  return (
    <section aria-label="Your rules">
      <SectionTitle title="Your rules" count={view.own.length}>
        <Button variant="ghost" className="h-7 px-2 text-[12px] text-accent! hover:text-accent!" onClick={() => useStyle.setState({ chatOpen: true })}>
          <Sparkles size={13} /> Talk it through with AI
        </Button>
      </SectionTitle>
      <div className="flex h-9 items-center gap-2 rounded-lg bg-white/[.06] px-3 shadow-[inset_0_0_0_1px_rgb(255_255_255/.08)] focus-within:shadow-[inset_0_0_0_1px_var(--color-accent)]">
        <Plus size={15} className="shrink-0 text-muted" />
        <input
          value={draft}
          maxLength={500}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              void add();
            }
          }}
          placeholder="Tell the AI what to do or not to do"
          aria-label="Tell the AI what to do or not to do"
          className="min-w-0 flex-1 bg-transparent text-[13px] text-fg outline-none placeholder:text-muted"
        />
        {draft.trim() && (
          <kbd aria-hidden className="rounded-[5px] bg-white/[.055] px-[5px] py-0.5 font-sans text-[11px] font-medium text-muted">
            ↵
          </kbd>
        )}
      </div>
      {view.own.length > 0 && (
        <ul className="mt-2 flex flex-col" aria-label="Your rules">
          {view.own.map((rule, i) =>
            editing === i ? (
              <li key={i} className="flex h-9 items-center rounded-lg bg-white/[.06] px-3 shadow-[inset_0_0_0_1px_var(--color-accent)]">
                <input
                  autoFocus
                  value={edit}
                  maxLength={500}
                  aria-label="Change your rule"
                  onChange={(e) => setEdit(e.target.value)}
                  onBlur={() => setEditing(null)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") {
                      e.preventDefault();
                      void save(i);
                    } else if (e.key === "Escape") {
                      e.preventDefault();
                      e.stopPropagation();
                      setEditing(null);
                    }
                  }}
                  className="min-w-0 flex-1 bg-transparent text-[13px] text-fg outline-none"
                />
              </li>
            ) : (
              <li
                key={i}
                tabIndex={0}
                aria-label={rule}
                onKeyDown={(e: KeyboardEvent<HTMLLIElement>) => {
                  if (e.target !== e.currentTarget) return;
                  if (e.key === "F2" || e.key === "Enter") {
                    e.preventDefault();
                    setEdit(rule);
                    editingWas.current = rule;
                    setEditing(i);
                  } else if (e.key === "Delete" || e.key === "Backspace") {
                    e.preventDefault();
                    remove(i);
                  }
                }}
                className={`group/rule flex min-h-9 items-center gap-2 rounded-lg px-3 py-1.5 text-[13px] text-fg outline-none hover:bg-white/[.05] focus-visible:bg-white/[.05] focus-visible:outline-2 focus-visible:outline-accent ${changed.has(`own:${rule}`) ? "changed" : ""}`}
              >
                <span className="min-w-0 flex-1">{rule}</span>
                <span className="flex shrink-0 gap-0.5 opacity-0 group-focus-within/rule:opacity-100 group-hover/rule:opacity-100">
                  <IconButton
                    label="Change (F2)"
                    onClick={() => {
                      setEdit(rule);
                      editingWas.current = rule;
                      setEditing(i);
                    }}
                  >
                    <Pencil size={14} />
                  </IconButton>
                  <IconButton label="Remove (Delete)" onClick={() => remove(i)}>
                    <Trash2 size={14} />
                  </IconButton>
                </span>
              </li>
            ),
          )}
        </ul>
      )}
    </section>
  );
}

/** A toast for a change of the style, with Undo back to `version`. */
function toastUndo(text: string, version: number) {
  useEditor.getState().toast({ kind: "success", text, action: undoTo(version) });
}

function LearnedRules({ view, changed }: { view: StyleView; changed: Set<string> }) {
  const [menu, setMenu] = useState<{ rule: StyleRule; at: { x: number; y: number; align: "end" }; keyboard: boolean; back: HTMLElement } | null>(null);
  if (view.rules.length === 0) return null;
  const items = (rule: StyleRule): MenuEntry[] => [
    { label: "Revert", run: () => void changeStyle({ type: "revert", title: rule.title }).then((done) => done && toastUndo(`Put back ${rule.title} as it was`, view.version)) },
    { label: "Remove", danger: true, run: () => void changeStyle({ type: "remove", title: rule.title }).then((done) => done && toastUndo(`Removed ${rule.title}`, view.version)) },
  ];
  return (
    <section aria-label="Learned rules">
      <SectionTitle title="Learned rules" count={view.rules.length} />
      <ul className="flex flex-col">
        {view.rules.map((rule) => (
          <li
            key={rule.title}
            className={`grid min-h-10 grid-cols-[148px_minmax(0,1fr)_auto_auto] items-center gap-3 rounded-lg px-3 py-1.5 hover:bg-white/[.04] ${changed.has(`rule:${rule.title}`) ? "changed" : ""}`}
          >
            <span className="truncate text-[13px] font-medium text-fg">{rule.title}</span>
            <span className="flex min-w-0 items-center gap-2 text-[13px] text-fg/85">
              <span className="truncate">{rule.summary || "As written in your style"}</span>
              {rule.byYou && (
                <span className="flex shrink-0 items-center gap-1 text-[12px] text-muted" title="You changed this rule by hand, so learning leaves it as it is">
                  <Pencil size={12} /> Changed by you
                </span>
              )}
            </span>
            {rule.confidence ? <Confidence c={rule.confidence} /> : <span className="text-[12px] text-muted">Not seen lately</span>}
            <IconButton
              label={`More for ${rule.title}`}
              aria-haspopup="menu"
              onClick={(e) => {
                const r = e.currentTarget.getBoundingClientRect();
                setMenu({ rule, at: { x: r.right, y: r.bottom + 4, align: "end" }, keyboard: e.detail === 0, back: e.currentTarget });
              }}
            >
              <MoreHorizontal size={16} />
            </IconButton>
          </li>
        ))}
      </ul>
      {menu && (
        <Menu
          label={menu.rule.title}
          at={menu.at}
          keyboard={menu.keyboard}
          items={items(menu.rule)}
          onClose={(chose) => {
            const back = menu.back;
            setMenu(null);
            if (!chose) back.focus();
          }}
        />
      )}
    </section>
  );
}

const REASON: Record<StyleFrozen, string> = { rejected: "Rejected", removed: "Removed", reverted: "Put back by you", edited: "Changed by you" };

function NotLearned({ view }: { view: StyleView }) {
  const [open, setOpen] = useState(false);
  const rules = view.notLearned.filter((n) => n.reason !== "edited");
  if (rules.length === 0) return null;
  return (
    <section aria-label="Not learned">
      <button type="button" aria-expanded={open} onClick={() => setOpen(!open)} className="mb-2 flex h-8 items-center gap-1.5 text-[13px] font-semibold text-fg">
        {open ? <ChevronDown size={15} className="text-muted" /> : <ChevronRight size={15} className="text-muted" />}
        Not learned <span className="tabular text-[12px] font-normal text-muted">{rules.length}</span>
      </button>
      {open && (
        <ul className="flex flex-col">
          {rules.map((rule) => (
            <li key={rule.title} className="flex min-h-10 items-center gap-3 rounded-lg px-3 py-1.5">
              <span className="text-[13px] text-fg">{rule.title}</span>
              <span className="flex-1 text-[12px] text-muted">{REASON[rule.reason]}</span>
              <Button pill onClick={() => void changeStyle({ type: "learnAgain", title: rule.title })}>
                Learn again
              </Button>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

function source(s: StyleSource) {
  return s.kind === "pair" ? `Your recording and finished video · matched ${Math.round((s.matched ?? 0) * 100)}%` : `Your edit of this project · ${when(s.atMs)}`;
}

function LearnedFrom({ view }: { view: StyleView }) {
  if (view.sources.length === 0) return null;
  return (
    <section aria-label="Learned from">
      <SectionTitle title="Learned from" count={view.sources.length} />
      <ul className="flex flex-col">
        {view.sources.map((s, i) => (
          <li key={i} className="flex min-h-10 flex-col justify-center rounded-lg px-3 py-1.5">
            <span className="truncate text-[13px] text-fg">{s.title}</span>
            <span className="text-[12px] text-muted">{source(s)}</span>
          </li>
        ))}
      </ul>
    </section>
  );
}

function VersionsButton({ view, onReset }: { view: StyleView; onReset: () => void }) {
  const [menu, setMenu] = useState<{ at: { x: number; y: number; align: "end" }; keyboard: boolean; back: HTMLElement } | null>(null);
  return (
    <>
      <IconButton
        label="Versions of your style"
        aria-haspopup="menu"
        aria-expanded={!!menu}
        disabled={view.versions.length === 0}
        onClick={(e) => {
          const r = e.currentTarget.getBoundingClientRect();
          setMenu({ at: { x: r.right, y: r.bottom + 6, align: "end" }, keyboard: e.detail === 0, back: e.currentTarget });
        }}
      >
        <History size={16} />
      </IconButton>
      {menu && (
        <Menu
          label="Versions of your style"
          minWidth={300}
          at={menu.at}
          keyboard={menu.keyboard}
          items={[
            ...view.versions.slice(0, 20).map((v) => ({
              label: v.label,
              shortcut: when(v.atMs),
              checked: v.index === view.version,
              run: v.index === view.version ? undefined : () => void changeStyle({ type: "restore", index: v.index }).then((done) => done && toastUndo(`Restored “${v.label}”`, view.version)),
            })),
            "separator" as const,
            { label: "Back to default…", danger: true, disabled: view.text === null && view.sources.length === 0 ? "Your style is the default already" : null, run: onReset },
          ]}
          onClose={(chose) => {
            const back = menu.back;
            setMenu(null);
            if (!chose) back.focus();
          }}
        />
      )}
    </>
  );
}

function ResetDialog({ view, onClose }: { view: StyleView; onClose: () => void }) {
  const dialog = useRef<HTMLDivElement>(null);
  useEffect(() => dialog.current?.querySelector("button")?.focus(), []);
  return (
    <div className="fixed inset-0 z-[100] flex items-center justify-center scrim-in bg-black/45" onPointerDown={(e) => e.target === e.currentTarget && onClose()}>
      <div
        ref={dialog}
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="reset-title"
        aria-describedby="reset-text"
        onKeyDown={(e) => {
          if (e.key !== "Escape") return;
          e.stopPropagation();
          onClose();
        }}
        className="overlay w-[420px] rounded-[20px] p-6 dialog-in"
      >
        <h2 id="reset-title" className="text-[17px] font-semibold leading-[22px] tracking-[-0.025em]">
          Go back to the default style?
        </h2>
        <p id="reset-text" className="mt-2 text-[13px] text-fg/85">
          Your AI goes back to Nuzky's defaults. What was learned from your videos is forgotten; your rules and the style itself come back from Versions.
        </p>
        <div className="mt-6 flex justify-end gap-2">
          <Button pill onClick={onClose}>
            Cancel
          </Button>
          <Button
            pill
            variant="danger"
            onClick={async () => {
              onClose();
              if (await changeStyle({ type: "reset" })) toastUndo("Your style is the default now", view.version);
            }}
          >
            Back to default
          </Button>
        </div>
      </div>
    </div>
  );
}

function TextEditor({ view, onDone }: { view: StyleView; onDone: () => void }) {
  // The text and the version it came from, as opened: a change made meanwhile is never overwritten.
  const [opened] = useState(() => ({ text: view.text ?? "", version: view.version }));
  const [text, setText] = useState(opened.text);
  const area = useRef<HTMLTextAreaElement>(null);
  const changed = text !== opened.text;
  useEffect(() => area.current?.focus(), []);
  const save = async () => {
    if (!changed) return onDone();
    if (await changeStyle({ type: "setText", text, baseVersion: opened.version }, "Saved your style")) onDone();
  };
  return (
    <div className="flex min-h-0 flex-1 flex-col gap-3 px-6 pb-6">
      <textarea
        ref={area}
        value={text}
        spellCheck={false}
        aria-label="EDIT.md"
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "s") {
            e.preventDefault();
            void save();
          } else if (e.key === "Escape" && !changed) {
            e.preventDefault();
            onDone();
          }
        }}
        className="min-h-0 flex-1 resize-none rounded-xl bg-black/25 p-4 font-mono text-[12.5px] leading-[1.55] text-fg outline-none shadow-[inset_0_0_0_1px_rgb(255_255_255/.08)] focus:shadow-[inset_0_0_0_1px_var(--color-accent)]"
      />
      <div className="flex items-center gap-2">
        <p className="flex-1 text-[12px] text-muted">The AI reads this file as it is. Learning leaves the rules you change here alone.</p>
        <Button pill onClick={onDone}>
          Cancel
        </Button>
        <Button pill variant="primary" onClick={() => void save()} title="Save (Ctrl+S)">
          Save
        </Button>
      </div>
    </div>
  );
}

function Empty() {
  return (
    <div className="mx-auto flex max-w-[420px] flex-col items-center py-16 text-center">
      <h2 className="text-[17px] font-semibold text-fg">Nuzky learns how you edit</h2>
      <p className="mt-2 text-[13px] text-muted">Export a video you cut, or show it videos you made before. Nothing leaves this computer.</p>
      <Button pill variant="primary" className="mt-5" onClick={() => useStyle.setState({ learnOpen: true })}>
        Learn from videos…
      </Button>
    </div>
  );
}

/** The creator's style: what the AI follows, what learning suggests, and the creator's own rules. */
export function StylePage() {
  const view = useStyle((s) => s.view);
  const error = useStyle((s) => s.error);
  const learnOpen = useStyle((s) => s.learnOpen);
  const [editing, setEditing] = useState(false);
  const [resetting, setResetting] = useState(false);
  const [busy, setBusy] = useState(false);
  const changed = useChanged(view);

  useEffect(() => {
    void loadStyle();
  }, []);

  const empty = view && view.text === null && view.suggestions.length === 0;
  const ruleCount = view ? view.rules.length + view.own.length : 0;
  return (
    <>
      <div className="flex h-16 shrink-0 items-center gap-2 pl-6 pr-4">
        <h1 className="shrink-0 text-[17px] font-semibold text-fg">Your style</h1>
        {ruleCount > 0 && <span className="tabular shrink-0 text-[13px] text-muted">{plural(ruleCount, "rule")}</span>}
        <div className="flex-1" />
        {view && !editing && (
          <>
            <VersionsButton view={view} onReset={() => setResetting(true)} />
            <Button variant="ghost" onClick={() => setEditing(true)} title="Read and change EDIT.md, the file the AI follows">
              <FileText size={14} /> Edit as text
            </Button>
            <Button variant={empty ? "primary" : "secondary"} onClick={() => useStyle.setState({ learnOpen: true })}>
              Learn from videos…
            </Button>
          </>
        )}
      </div>
      {error && !view ? (
        <div className="flex flex-1 flex-col items-center justify-center gap-3 text-center text-[13px] text-muted">
          <AlertCircle size={28} className="text-danger" />
          <span>Your style couldn't be read: {error}</span>
          <Button pill onClick={() => void loadStyle()}>
            Try again
          </Button>
        </div>
      ) : !view ? null : editing ? (
        <TextEditor view={view} onDone={() => setEditing(false)} />
      ) : (
        <div className="min-h-0 flex-1 overflow-y-auto px-6 pb-24">
          <div className="flex max-w-[920px] flex-col gap-8">
            {empty && <Empty />}
            {view.suggestions.length > 0 && (
              <section aria-label="Suggestions">
                <SectionTitle title="Suggestions" count={view.suggestions.length}>
                  {view.suggestions.length > 1 && (
                    <Button
                      variant="primary"
                      disabled={busy}
                      onClick={async () => {
                        setBusy(true);
                        await changeStyle({ type: "accept", titles: view.suggestions.map((s) => s.title) });
                        setBusy(false);
                      }}
                    >
                      Accept all
                    </Button>
                  )}
                </SectionTitle>
                <div className="flex flex-col gap-2">
                  {view.suggestions.map((s) => (
                    <SuggestionCard key={s.title} s={s} busy={busy} />
                  ))}
                </div>
              </section>
            )}
            <OwnRules view={view} changed={changed} />
            <LearnedRules view={view} changed={changed} />
            <NotLearned view={view} />
            <LearnedFrom view={view} />
          </div>
        </div>
      )}
      {learnOpen && <LearnDialog />}
      {resetting && view && <ResetDialog view={view} onClose={() => setResetting(false)} />}
    </>
  );
}
