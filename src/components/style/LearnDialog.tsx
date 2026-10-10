import { useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { AlertTriangle, ArrowRight, Check, Film, Plus, X } from "lucide-react";
import { api, plainError } from "../../lib/api";
import { NO_PAIR, addProjects, fileName, loadStyle, openStyle, saveLearned, startLearning, startProjectsLearning, useStyle } from "../../lib/style";
import { useEditor } from "../../lib/store";
import { formatLength } from "../../lib/time";
import type { StylePair, StyleTimelineResult, TimelinePlan } from "../../lib/types";
import { AiLock, Button, IconButton, ProgressBar, TabBar, TabPanel, trapTab } from "../ui";

const VIDEO_EXTENSIONS = ["mp4", "mov", "m4v", "mkv", "webm", "avi", "mts"];

async function pickVideo(): Promise<string | null> {
  const picked = await open({ multiple: false, filters: [{ name: "Video", extensions: VIDEO_EXTENSIONS }] });
  return typeof picked === "string" ? picked : null;
}

async function pickProjects() {
  const picked = await open({ multiple: true, filters: [{ name: "OpenTimelineIO", extensions: ["otio"] }] });
  if (picked?.length) await addProjects(picked);
}

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

function FileButton({ path, label, disabled, onPick }: { path: string; label: string; disabled: boolean; onPick: (path: string) => void }) {
  return (
    <Button
      className="h-8 min-w-0 flex-1 justify-start gap-2 px-2.5"
      aria-label={path ? `${label}: ${fileName(path)}` : `Choose the ${label.toLowerCase()}`}
      title={path || undefined}
      disabled={disabled}
      disabledReason="Learning is running"
      onClick={async () => {
        const picked = await pickVideo();
        if (picked) onPick(picked);
      }}
    >
      <Film size={14} className="shrink-0 text-muted" />
      <span className={`truncate ${path ? "text-fg" : "text-muted"}`}>{path ? fileName(path) : "Choose…"}</span>
    </Button>
  );
}

/** A row's phase and percentage while learning works on it; none while the bar can only say it is working. */
function Running({ phase, progress, label }: { phase: string | null | undefined; progress: number; label: string }) {
  return (
    <div className="flex w-full flex-col gap-1.5">
      <span className="flex justify-between gap-2 text-fg">
        <span className="truncate">{phase ?? "Starting"}</span>
        {progress > 0 && <span className="tabular text-muted">{Math.round(progress * 100)}%</span>}
      </span>
      <ProgressBar value={progress} label={label} />
    </div>
  );
}

function Warning({ text }: { text: string }) {
  return (
    <span className="flex items-start gap-1.5 text-fg" title={text}>
      <AlertTriangle size={14} className="mt-px shrink-0 text-warn" />
      <span className="line-clamp-2">{text}</span>
    </span>
  );
}

/** Learns the style from videos the creator cut before: a recording with the finished video cut from it, or projects cut in another editor. */
export function LearnDialog() {
  const tab = useStyle((s) => s.learnTab);
  const dialog = useRef<HTMLDivElement>(null);
  const close = () => useStyle.setState({ learnOpen: false });

  useEffect(() => {
    dialog.current?.querySelector<HTMLElement>("[data-autofocus]")?.focus();
  }, []);

  return (
    <div className="fixed inset-0 z-[100] flex items-center justify-center scrim-in bg-black/45" onPointerDown={(e) => e.target === e.currentTarget && close()}>
      <div
        ref={dialog}
        role="dialog"
        aria-modal="true"
        aria-labelledby="learn-title"
        onKeyDown={(e) => {
          trapTab(e);
          if (e.key === "Escape") {
            // Esc closes only this, never the home screen behind it.
            e.preventDefault();
            e.stopPropagation();
            close();
          }
        }}
        className="overlay flex max-h-[calc(100vh-48px)] w-[640px] max-w-[calc(100vw-48px)] flex-col rounded-[20px] p-6 dialog-in"
      >
        <div className="mb-4 flex shrink-0 items-start justify-between gap-3">
          <h2 id="learn-title" className="text-[17px] font-semibold leading-[22px] tracking-[-0.025em]">
            Learn my style
          </h2>
          <IconButton label="Close" round className="-mr-2 -mt-1" onClick={close}>
            <X size={16} />
          </IconButton>
        </div>
        <TabBar
          group="learn"
          label="Learn from"
          className="mb-5"
          tabs={[
            { id: "videos", label: "Videos" },
            { id: "projects", label: "Projects (.otio)" },
          ]}
          value={tab}
          onChange={(learnTab) => useStyle.setState({ learnTab })}
        />
        <TabPanel group="learn" id={tab} className="flex min-h-0 flex-1 flex-col">
          {tab === "videos" ? <Videos /> : <Projects />}
        </TabPanel>
      </div>
    </div>
  );
}

/** 1 to 3 recordings, each with the finished video cut from it. */
function Videos() {
  const learning = useStyle((s) => s.learning);
  const job = useEditor((s) => (learning ? s.jobs[learning.jobId] : undefined));
  const running = job?.status === "running";
  const ended = !!learning && !!job && !running;
  const rows = useStyle((s) => s.draft);
  // Choosing other files after learning ended leaves its results, which were about the old ones.
  const setRows = (change: (rows: StylePair[]) => StylePair[]) => useStyle.setState((s) => ({ draft: change(s.draft), ...(running ? {} : { learning: null }) }));
  const pairs = learning && (running || ended) ? learning.pairs : rows;
  const close = () => useStyle.setState({ learnOpen: false });

  const set = (i: number, side: keyof StylePair, path: string) => setRows((all) => all.map((r, k) => (k === i ? { ...r, [side]: path } : r)));
  const ready = rows.length > 0 && rows.every((r) => r.recording && r.cut);
  const runningRow = learning?.results.findIndex((r) => r === null) ?? -1;
  // The job's progress is over every pair; each row shows its own part.
  const rowProgress = job ? Math.min(1, Math.max(0, job.progress * pairs.length - runningRow)) : 0;

  const learn = () => {
    if (!ready) return;
    useStyle.setState({ learning: null });
    void startLearning(rows);
  };

  return (
    <>
      <div className="grid grid-cols-[1fr_16px_1fr_196px] items-center gap-x-2.5 gap-y-2">
        <span className="text-[12px] text-muted">Recording</span>
        <span />
        <span className="text-[12px] text-muted">Finished video</span>
        <span />
        {pairs.map((pair, i) => {
          const result = learning && (running || ended) ? learning.results[i] : null;
          return (
            <div key={i} role="group" aria-label={`Pair ${i + 1}`} className="contents">
              <FileButton path={pair.recording} label="Recording" disabled={running} onPick={(p) => set(i, "recording", p)} />
              <ArrowRight size={14} className="text-muted" aria-hidden />
              <FileButton path={pair.cut} label="Finished video" disabled={running} onPick={(p) => set(i, "cut", p)} />
              <div className="flex min-h-8 min-w-0 items-center text-[12px]" aria-live="polite">
                {result?.error ? (
                  <Warning text={plainError(result.error)} />
                ) : result ? (
                  <span className="flex items-center gap-1.5 text-fg">
                    <Check size={14} className="shrink-0 text-ok" />
                    Matched {Math.round((result.matched ?? 0) * 100)}%
                  </span>
                ) : running && i === runningRow ? (
                  <Running phase={job?.phase} progress={rowProgress} label={`Pair ${i + 1}`} />
                ) : running ? (
                  <span className="text-muted">Waiting</span>
                ) : !learning || !ended ? (
                  rows.length > 1 && (
                    <IconButton label={`Remove pair ${i + 1}`} onClick={() => setRows((all) => all.filter((_, k) => k !== i))}>
                      <X size={15} />
                    </IconButton>
                  )
                ) : null}
              </div>
            </div>
          );
        })}
      </div>
      {!running && !ended && rows.length < 3 && (
        <Button className="mt-3 h-8 gap-1.5 self-start px-2.5" onClick={() => setRows((all) => [...all, NO_PAIR])}>
          <Plus size={14} /> Add a pair
        </Button>
      )}
      {ended && job?.status === "failed" && (
        <p className="mt-4 flex items-start gap-1.5 text-[12px] text-fg" role="alert">
          <AlertTriangle size={14} className="mt-px shrink-0 text-danger" />
          {plainError(job.message ?? "Learning failed.")}
        </p>
      )}
      <div className="mt-6 flex items-center gap-2 border-t border-white/[.08] pt-4">
        <p className="flex-1 text-[12px] text-muted">
          {running ? "Keeps running if you close this." : "Each finished video is compared with its recording here, on this computer."}
        </p>
        {running ? (
          <Button pill data-autofocus onClick={() => job && void api.cancelJob(job.id)}>
            Stop
          </Button>
        ) : ended && job?.status === "done" ? (
          <>
            <Button
              pill
              onClick={() => {
                useStyle.setState({ learning: null });
                setRows(() => [NO_PAIR]);
              }}
            >
              Learn from more
            </Button>
            <Button pill variant="primary" data-autofocus onClick={close}>
              Review
            </Button>
          </>
        ) : (
          <Button pill variant="primary" data-autofocus disabled={!ready} disabledReason="Choose a recording and the video you cut from it" onClick={learn}>
            Learn
          </Button>
        )}
      </div>
    </>
  );
}

/** What a project will teach before learning: its recordings, what is missing and what is not read. */
function Plan({ plan }: { plan: TimelinePlan }) {
  if (plan.error) return <Warning text={plainError(plan.error)} />;
  return (
    <>
      <span className="text-muted">
        {plan.recordings.map((r, i) => (
          <span key={r.path}>
            {i > 0 && ", "}
            <span className={r.missing ? "text-fg" : undefined} title={r.path}>
              {r.name}
            </span>
            {r.missing ? <span className="text-warn">: {r.missing}</span> : ` · ${plural(r.clips, "clip")}`}
          </span>
        ))}
        {` · ${formatLength(plan.durationUs)}`}
        {plan.texts > 0 && ` · ${plural(plan.texts, "title")}`}
      </span>
      {plan.unread.length > 0 && (
        <span className="truncate text-muted" title={plan.unread.join("\n")}>
          Not read: {plan.unread.join("; ")}
        </span>
      )}
    </>
  );
}

/** How learning from a project ended, without repeating what its row already says is missing. */
function Outcome({ plan, result }: { plan: TimelinePlan; result: StyleTimelineResult }) {
  if (result.error) return <Warning text={plainError(result.error)} />;
  const told = new Set(plan.recordings.filter((r) => r.missing).map((r) => r.name));
  const why = result.skipped.filter(([name]) => !told.has(name)).map(([name, reason]) => `${name}: ${reason}`);
  if (result.learned.length === 0) return <Warning text={why.join("; ") || "Nothing to learn"} />;
  return (
    <span className="flex min-w-0 items-center gap-1.5 text-fg" title={why.join("\n") || undefined}>
      <Check size={14} className="shrink-0 text-ok" />
      <span className="truncate">Learned{why.length > 0 && `, ${plural(why.length, "recording")} skipped`}</span>
    </span>
  );
}

/** Timelines the creator cut in another editor, as OpenTimelineIO files. */
function Projects() {
  const learning = useStyle((s) => s.projectsLearning);
  const job = useEditor((s) => (learning ? s.jobs[learning.jobId] : undefined));
  const running = job?.status === "running";
  const ended = !!learning && !!job && !running;
  const draft = useStyle((s) => s.projects);
  const plans = learning && (running || ended) ? learning.plans : draft;
  const view = useStyle((s) => s.view);
  const [confirming, setConfirming] = useState(false);
  const [saving, setSaving] = useState(false);
  const panel = useRef<HTMLDivElement>(null);
  const preview = ended && job?.status === "done" ? job.output : null;
  const replacing = !!view?.text;

  useEffect(() => {
    void loadStyle();
  }, []);
  // A button that went away with the state, such as Stop or Choose projects, leaves focus on the action that replaced it.
  const [empty, previewed] = [plans.length === 0, !!preview];
  useEffect(() => {
    const lost = !document.activeElement || document.activeElement === document.body;
    if (lost) panel.current?.querySelector<HTMLElement>("[data-autofocus]")?.focus();
  }, [empty, running, previewed, confirming]);

  // Another choice of projects after learning ended leaves its results, which were about the old ones.
  const setDraft = (change: (plans: TimelinePlan[]) => TimelinePlan[]) => useStyle.setState((s) => ({ projects: change(s.projects), projectsLearning: null }));
  const choose = async () => {
    useStyle.setState({ projectsLearning: null });
    await pickProjects();
  };
  const editable = !running && !preview;
  const ready = draft.some((p) => !p.error && p.recordings.some((r) => !r.missing));
  const runningRow = learning?.results.findIndex((r) => r === null) ?? -1;
  const rowProgress = job ? Math.min(1, Math.max(0, job.progress * plans.length - runningRow)) : 0;

  const save = async () => {
    if (!learning) return;
    setSaving(true);
    const saved = await saveLearned(learning.jobId);
    setSaving(false);
    setConfirming(false);
    if (saved) {
      useStyle.setState({ learnOpen: false });
      openStyle();
    }
  };

  return (
    <div ref={panel} className="contents">
      {plans.length === 0 ? (
        <div className="flex flex-col items-center gap-4 px-6 py-8 text-center">
          <p className="max-w-[440px] text-[13px] text-muted">
            Export each timeline as OpenTimelineIO (.otio), which DaVinci Resolve and other editors can write. The recordings it uses must be on this computer.
          </p>
          <Button pill variant="primary" data-autofocus onClick={() => void choose()}>
            Choose projects…
          </Button>
        </div>
      ) : (
        <ul aria-label="Projects" className={`flex min-h-0 flex-col overflow-y-auto ${preview ? "max-h-[132px] shrink-0" : ""}`}>
          {plans.map((plan, i) => {
            const result = learning && (running || ended) ? learning.results[i] : null;
            return (
              <li key={plan.path} className="flex items-center gap-3 border-b border-white/[.06] py-2 last:border-b-0">
                <div className="flex min-w-0 flex-1 flex-col text-[12px]">
                  <span className="truncate text-[13px] font-medium text-fg" title={plan.path}>
                    {plan.name}
                  </span>
                  <Plan plan={plan} />
                </div>
                <div className="flex min-h-8 w-[196px] shrink-0 items-center justify-end text-[12px]" aria-live="polite">
                  {result ? (
                    <Outcome plan={plan} result={result} />
                  ) : running && i === runningRow ? (
                    <Running phase={job?.phase} progress={rowProgress} label={plan.name} />
                  ) : running ? (
                    <span className="text-muted">Waiting</span>
                  ) : editable ? (
                    <IconButton label={`Remove ${plan.name}`} onClick={() => setDraft((all) => all.filter((p) => p.path !== plan.path))}>
                      <X size={15} />
                    </IconButton>
                  ) : null}
                </div>
              </li>
            );
          })}
        </ul>
      )}
      {editable && plans.length > 0 && (
        <Button className="mt-3 h-8 gap-1.5 self-start px-2.5" onClick={() => void choose()}>
          <Plus size={14} /> Add projects
        </Button>
      )}
      {preview && (
        <section aria-label="EDIT.md as learned" className="mt-4 flex min-h-[120px] flex-1 flex-col gap-1.5">
          <span className="text-[12px] text-muted">EDIT.md, the style the AI will follow</span>
          <pre
            tabIndex={0}
            className="min-h-0 flex-1 overflow-auto whitespace-pre-wrap rounded-xl bg-black/25 p-4 font-mono text-[12.5px] leading-[1.55] text-fg shadow-[inset_0_0_0_1px_rgb(255_255_255/.08)] outline-none focus-visible:shadow-[inset_0_0_0_1px_var(--color-accent)]"
          >
            {preview}
          </pre>
        </section>
      )}
      {ended && job?.status === "failed" && (
        <p className="mt-4 flex items-start gap-1.5 text-[12px] text-fg" role="alert">
          <AlertTriangle size={14} className="mt-px shrink-0 text-danger" />
          {plainError(job.message ?? "Learning failed.")}
        </p>
      )}
      {plans.length > 0 && (
        <div
          className="mt-6 flex shrink-0 items-center gap-2 border-t border-white/[.08] pt-4"
          onKeyDown={(e) => {
            // Esc answers the question, and leaves the dialog open.
            if (confirming && e.key === "Escape") {
              e.preventDefault();
              e.stopPropagation();
              setConfirming(false);
            }
          }}
        >
          <p className="flex-1 text-[12px] text-muted" role={confirming ? "alert" : undefined}>
            {running
              ? "Keeps running if you close this."
              : confirming
                ? "Replace your style with this one? Its learned rules, also those you changed by hand, give way; Your rules stay. Versions bring the old style back."
                : preview
                  ? replacing
                    ? ""
                    : "Nothing changes until you use it."
                  : "Each project's recordings are compared with its cut here, on this computer."}
          </p>
          {running ? (
            <Button pill data-autofocus onClick={() => job && void api.cancelJob(job.id)}>
              Stop
            </Button>
          ) : confirming ? (
            <>
              <Button key="cancel" pill data-autofocus onClick={() => setConfirming(false)}>
                Cancel
              </Button>
              <AiLock>
                <Button key="replace" pill variant="primary" disabled={saving} onClick={() => void save()}>
                  Replace
                </Button>
              </AiLock>
            </>
          ) : preview ? (
            <>
              <Button pill onClick={() => useStyle.setState({ projectsLearning: null })}>
                Learn from more
              </Button>
              <AiLock>
                <Button key="use" pill variant="primary" data-autofocus disabled={saving || !view} onClick={() => (replacing ? setConfirming(true) : void save())}>
                  {replacing ? "Replace my style…" : "Use as my style"}
                </Button>
              </AiLock>
            </>
          ) : (
            <Button
              pill
              variant="primary"
              data-autofocus
              disabled={!ready}
              disabledReason="Choose a project whose recordings are on this computer"
              onClick={() => void startProjectsLearning(draft)}
            >
              Learn
            </Button>
          )}
        </div>
      )}
    </div>
  );
}
