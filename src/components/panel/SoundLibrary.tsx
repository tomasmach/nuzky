import { AudioLines, Check, Copy, ExternalLink, Loader2, Pause, Play, Plus, Search, SlidersHorizontal, WifiOff, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { api, errorText, plainError } from "../../lib/api";
import { addSound, copyCredits, loadBuiltIn, searchSounds, stopPreview, togglePreview, useSounds } from "../../lib/sounds";
import { useEditor } from "../../lib/store";
import { formatDuration } from "../../lib/time";
import type { Sound, SoundKind, SoundSettings } from "../../lib/types";
import { Button, Checkbox, IconButton, lockedProps, trapTab, useLockReason } from "../ui";

/** Round row action, as on the project's own audio. */
const ACTION = "flex h-7 w-7 shrink-0 items-center justify-center rounded-full transition-colors duration-[120ms] ease-out aria-disabled:cursor-not-allowed aria-disabled:opacity-40";
const MUSIC_IDEAS = ["Upbeat", "Chill", "Cinematic", "Happy", "Lofi", "Epic", "Acoustic", "Electronic"];
const FREESOUND_KEYS = "https://freesound.org/apiv2/apply/";

/** One sound: the picture plays it, + adds it at the playhead, the licence opens its page at the source. */
function SoundRow({ sound, inProject }: { sound: Sound; inProject: boolean }) {
  const preview = useSounds((s) => (s.preview?.id === sound.id ? s.preview : null));
  const adding = useSounds((s) => s.adding.includes(sound.id));
  const progress = useSounds((s) => s.progress[sound.id] ?? 0);
  const lock = useLockReason();
  const online = sound.provider !== "Built in";
  const license = sound.license === "by" ? "CC BY" : "CC0";
  return (
    <div
      data-sound={sound.id}
      onDoubleClick={() => !lock && addSound(sound)}
      className="group relative flex items-center gap-2.5 rounded-xl p-1.5 focus-within:bg-raised hover:bg-raised"
    >
      <button
        type="button"
        aria-label={preview ? `Stop ${sound.title}` : `Play ${sound.title}`}
        aria-pressed={!!preview}
        title={preview ? "Stop" : "Play"}
        onClick={() => togglePreview(sound)}
        className={`flex h-10 w-10 shrink-0 items-center justify-center rounded-md text-fg ${sound.kind === "music" ? "bg-clip-audio" : "bg-white/[.07]"}`}
      >
        {preview?.loading ? (
          <Loader2 size={16} className="animate-spin" />
        ) : preview ? (
          <Pause size={16} fill="currentColor" />
        ) : (
          <>
            <AudioLines size={18} className="group-focus-within:hidden group-hover:hidden" />
            <Play size={16} fill="currentColor" className="hidden group-focus-within:block group-hover:block" />
          </>
        )}
      </button>
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        <span className="truncate text-[13px] text-fg">{sound.title}</span>
        <span className="truncate text-[12px] text-muted">{online ? `${sound.author} · ${sound.provider}` : sound.category}</span>
      </div>
      {online && (
        <button
          type="button"
          onClick={() => api.openSoundPage(sound.url).catch((e) => useEditor.getState().toast({ kind: "error", text: errorText(e) }))}
          title={`${license} ${sound.licenseVersion}${sound.license === "by" ? ", needs credit" : ", no credit needed"}. Open its page at ${sound.provider} to check the licence.`}
          className="shrink-0 rounded-[4px] px-[5px] py-px text-[11px] font-medium text-muted shadow-[inset_0_0_0_1px_rgb(255_255_255/.14)] hover:text-fg"
        >
          {license}
        </button>
      )}
      <span className="tabular shrink-0 text-[12px] text-muted">{sound.durationUs > 0 ? formatDuration(sound.durationUs) : ""}</span>
      {adding ? (
        <button type="button" aria-label={`Stop adding ${sound.title}`} title="Downloading. Click to stop." onClick={() => api.soundCancel(sound.id)} className={`text-muted hover:text-fg ${ACTION}`}>
          {progress > 0 && progress < 1 ? <span className="tabular text-[10px]">{Math.round(progress * 100)}</span> : <Loader2 size={15} className="animate-spin" />}
        </button>
      ) : (
        <>
          {inProject && (
            <span className="flex h-7 w-7 shrink-0 items-center justify-center text-ok group-focus-within:hidden group-hover:hidden" title="In this project">
              <Check size={16} />
            </span>
          )}
          <button
            type="button"
            aria-label={`Add ${sound.title} at playhead`}
            title={inProject ? "In this project. Add it again at the playhead" : "Add at playhead"}
            onClick={() => addSound(sound)}
            {...lockedProps(lock)}
            className={`btn-prominent ${ACTION} ${inProject ? "hidden group-focus-within:flex group-hover:flex" : "opacity-0 group-focus-within:opacity-100 group-hover:opacity-100"}`}
          >
            <Plus size={16} />
          </button>
        </>
      )}
    </div>
  );
}

/** Where online results come from, and the user's own Freesound key. */
function Sources({ onClose }: { onClose: () => void }) {
  const [settings, setSettings] = useState<SoundSettings | null>(null);
  const [key, setKey] = useState("");
  const [error, setError] = useState<string | null>(null);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    api.soundSettings().then(setSettings);
    ref.current?.focus();
    const outside = (e: PointerEvent) => !ref.current?.contains(e.target as Node) && !(e.target as Element).closest?.("[data-sources-button]") && onClose();
    window.addEventListener("pointerdown", outside, true);
    return () => window.removeEventListener("pointerdown", outside, true);
  }, [onClose]);
  const save = async (freesound: boolean, newKey?: string) => {
    try {
      setSettings(await api.setSoundSettings(freesound, newKey));
      setError(null);
      if (newKey !== undefined) setKey("");
      useSounds.setState((s) => ({ search: { ...s.search, effect: { ...s.search.effect, sounds: [], page: 0, service: null } } }));
      const query = useSounds.getState().search.effect.query;
      if (query.trim()) searchSounds("effect", query);
    } catch (e) {
      setError(plainError(errorText(e)));
    }
  };
  return (
    <div
      ref={ref}
      tabIndex={-1}
      role="dialog"
      aria-label="Where to search"
      onKeyDown={(e) => {
        e.stopPropagation();
        if (e.key === "Escape") onClose();
        trapTab(e);
      }}
      className="overlay absolute top-[44px] right-3.5 z-20 flex w-[300px] flex-col gap-3 rounded-[12px] p-3 text-[12px] outline-none"
    >
      <p className="text-[13px] font-semibold text-fg">Where to search</p>
      <div className="flex flex-col gap-1">
        <p className="text-[13px] text-fg">Openverse</p>
        <p className="text-muted">Music and sounds from Jamendo, Freesound and Wikimedia Commons. Only CC0 and CC BY.</p>
      </div>
      <div className="flex flex-col gap-2 border-t border-white/[.07] pt-3">
        {settings && <Checkbox label="Freesound for sound effects" checked={settings.freesound} onChange={(on) => save(on)} />}
        <p className="text-muted">Needs your own free API key. Freesound allows its API for non-commercial use only.</p>
        {settings?.hasKey ? (
          <div className="flex items-center justify-between gap-2">
            <span className="text-fg">Your key is saved</span>
            <Button className="h-7 text-[12px]" onClick={() => save(settings.freesound, "")}>
              Remove key
            </Button>
          </div>
        ) : (
          <form
            className="flex gap-2"
            onSubmit={(e) => {
              e.preventDefault();
              if (key.trim()) save(true, key.trim());
            }}
          >
            <input
              type="password"
              value={key}
              onChange={(e) => setKey(e.target.value)}
              placeholder="Paste your Freesound API key"
              aria-label="Freesound API key"
              autoComplete="off"
              className="h-7 min-w-0 flex-1 rounded-[6px] border border-white/[.08] bg-white/[.055] px-2 text-[12px] text-fg outline-none placeholder:text-muted focus:border-accent"
            />
            <Button type="submit" className="h-7 text-[12px]" disabled={!key.trim()}>
              Save
            </Button>
          </form>
        )}
        {error && <p className="text-danger">{error}</p>}
        <button type="button" onClick={() => api.openSoundPage(FREESOUND_KEYS)} className="flex items-center gap-1 self-start font-medium text-accent hover:underline">
          Get a key <ExternalLink size={12} />
        </button>
      </div>
      <p className="border-t border-white/[.07] pt-3 text-muted">Searching sends only what you type, never your project.</p>
    </div>
  );
}

function SearchField({ kind }: { kind: SoundKind }) {
  const query = useSounds((s) => s.search[kind].query);
  const timer = useRef<number | undefined>(undefined);
  useEffect(() => () => window.clearTimeout(timer.current), []);
  // Each search counts against Openverse's limit, so typing waits for a pause.
  const change = (text: string, now = false) => {
    window.clearTimeout(timer.current);
    useSounds.setState((s) => ({ search: { ...s.search, [kind]: { ...s.search[kind], query: text } } }));
    if (now || !text.trim()) searchSounds(kind, text);
    else if (text.trim().length >= 2) timer.current = window.setTimeout(() => searchSounds(kind, text), 600);
  };
  return (
    <label className="relative flex h-8 min-w-0 flex-1 items-center">
      <Search size={14} className="pointer-events-none absolute left-2.5 text-muted" />
      <input
        type="search"
        value={query}
        placeholder={kind === "music" ? "Search free music" : "Search sound effects"}
        aria-label={kind === "music" ? "Search free music" : "Search sound effects"}
        onChange={(e) => change(e.target.value)}
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === "Enter") change(query, true);
          else if (e.key === "Escape" && query) change("");
        }}
        className={`h-8 w-full text-ellipsis rounded-lg border border-white/[.08] bg-white/[.055] pl-8 text-[13px] text-fg outline-none placeholder:text-muted focus:border-accent [&::-webkit-search-cancel-button]:hidden ${query ? "pr-7" : "pr-2"}`}
      />
      {query && (
        <button type="button" aria-label="Clear search" onClick={() => change("")} className="absolute right-1.5 flex h-5 w-5 items-center justify-center rounded-full text-muted hover:text-fg">
          <X size={13} />
        </button>
      )}
    </label>
  );
}

/** Ids of the library sounds the project already has. */
function useInProject() {
  const assets = useEditor((s) => s.snap?.project.assets);
  return new Set((assets ?? []).flatMap((a) => (a.credit ? [`${a.credit.source}:${a.credit.id}`] : [])));
}

function OnlineResults({ kind }: { kind: SoundKind }) {
  const search = useSounds((s) => s.search[kind]);
  const inProject = useInProject();
  if (!search.query.trim()) return null;
  const offline = search.error?.startsWith("OFFLINE");
  return (
    <section aria-label="Online results" className="flex flex-col gap-0.5">
      {kind === "effect" && <h3 className="mx-1.5 mt-2 mb-1 text-[13px] font-semibold text-fg">Online</h3>}
      {search.sounds.map((sound) => (
        <SoundRow key={sound.id} sound={sound} inProject={inProject.has(sound.id)} />
      ))}
      {search.loading &&
        [0, 1, 2].map((i) => (
          <div key={i} className="flex items-center gap-2.5 p-1.5" role="status" aria-label="Searching">
            <div className="skeleton h-10 w-10 shrink-0 rounded-md" />
            <div className="flex flex-1 flex-col gap-1.5">
              <div className="skeleton h-3 w-2/3 rounded" />
              <div className="skeleton h-2.5 w-1/3 rounded" />
            </div>
          </div>
        ))}
      {search.error && (
        <div className="mx-1.5 my-2 flex flex-col items-center gap-2 rounded-xl p-4 text-center" role="alert">
          {offline && <WifiOff size={24} className="text-muted" />}
          <p className="text-[13px] font-semibold text-fg">{offline ? "You're offline" : "Search didn't work"}</p>
          <p className="text-[12px] text-muted">
            {offline ? `${kind === "music" ? "Music search" : "Online search"} needs the internet.${kind === "effect" ? " Built-in sounds still work." : ""}` : plainError(search.error)}
          </p>
          <Button className="mt-1 h-7 text-[12px]" onClick={() => searchSounds(kind, search.query)}>
            Try again
          </Button>
        </div>
      )}
      {!search.loading && !search.error && search.page > 0 && search.sounds.length === 0 && (
        <p className="mx-1.5 my-3 text-[12px] text-muted">Nothing free to use in videos matches “{search.query.trim()}”. Try a simpler word in English.</p>
      )}
      {search.more && !search.loading && (
        <Button className="mx-1.5 mt-1 h-7 text-[12px]" onClick={() => searchSounds(kind, search.query, search.page + 1)}>
          More results
        </Button>
      )}
      {search.service && (
        <p className="mx-1.5 mt-3 text-[11px] leading-[15px] text-muted">
          {search.service === "Openverse" ? "Search uses Openverse. Nuzky is not endorsed or certified by Openverse." : "Results from Freesound with your API key."} Licence data can be wrong, so check a sound's page before you publish.
        </p>
      )}
    </section>
  );
}

function Effects() {
  const builtIn = useSounds((s) => s.builtIn);
  const query = useSounds((s) => s.search.effect.query.trim().toLowerCase());
  const [category, setCategory] = useState<string | null>(null);
  const inProject = useInProject();
  const categories = [...new Set(builtIn.map((s) => s.category ?? ""))];
  const shown = builtIn.filter((s) =>
    query ? [s.title, s.category ?? "", s.id].some((t) => t.toLowerCase().includes(query)) : !category || s.category === category,
  );
  return (
    <>
      {!query && (
        <div role="group" aria-label="Kinds of sound effects" className="flex shrink-0 flex-wrap gap-1.5 px-3.5 pb-2">
          {[null, ...categories].map((c) => (
            <button
              key={c ?? "all"}
              type="button"
              aria-pressed={category === c}
              onClick={() => setCategory(c)}
              className={`h-[26px] rounded-full px-2.5 text-[12px] font-medium transition-colors duration-[120ms] ease-out ${
                category === c ? "bg-accent/[.18] text-fg shadow-[inset_0_0_0_1px_rgb(41_151_255/.5)]" : "bg-white/[.06] text-muted hover:text-fg"
              }`}
            >
              {c ?? "All"}
            </button>
          ))}
        </div>
      )}
      <div className="flex min-h-0 flex-1 flex-col gap-0.5 overflow-y-auto px-2 pb-3.5">
        {shown.length > 0 && (
          <section aria-label="Built in" className="flex flex-col gap-0.5">
            <h3 className="mx-1.5 mb-1 flex items-baseline gap-1.5 text-[13px] font-semibold text-fg">
              Built in <span className="tabular text-[12px] font-normal text-muted">{shown.length}</span>
            </h3>
            {shown.map((sound) => (
              <SoundRow key={sound.id} sound={sound} inProject={inProject.has(sound.id)} />
            ))}
          </section>
        )}
        <OnlineResults kind="effect" />
      </div>
    </>
  );
}

function Music() {
  const query = useSounds((s) => s.search.music.query);
  if (!query.trim())
    return (
      <div className="flex min-h-0 flex-1 flex-col items-center justify-center gap-3 px-6 pb-6 text-center">
        <p className="text-[13px] text-muted">Free music you can use in videos you monetize.</p>
        <div className="flex flex-wrap justify-center gap-1.5">
          {MUSIC_IDEAS.map((idea) => (
            <button key={idea} type="button" onClick={() => searchSounds("music", idea)} className="h-[26px] rounded-full bg-white/[.06] px-2.5 text-[12px] font-medium text-muted transition-colors duration-[120ms] ease-out hover:text-fg">
              {idea}
            </button>
          ))}
        </div>
      </div>
    );
  return (
    <div className="flex min-h-0 flex-1 flex-col gap-0.5 overflow-y-auto px-2 pb-3.5">
      <OnlineResults kind="music" />
    </div>
  );
}

/** Music or sound effects to search, preview and add at the playhead. */
export function SoundLibrary({ kind }: { kind: SoundKind }) {
  const [sources, setSources] = useState(false);
  useEffect(() => {
    loadBuiltIn().catch(() => {});
    return () => {
      if (useSounds.getState().preview) stopPreview();
    };
  }, []);
  return (
    <div className="relative flex min-h-0 flex-1 flex-col">
      <div className="flex shrink-0 items-center gap-1.5 px-3.5 pt-1 pb-2">
        <SearchField kind={kind} />
        <IconButton label="Where to search" data-sources-button active={sources} onClick={() => setSources((open) => !open)}>
          <SlidersHorizontal size={16} />
        </IconButton>
      </div>
      {sources && <Sources onClose={() => setSources(false)} />}
      {kind === "music" ? <Music /> : <Effects />}
    </div>
  );
}

/** How many CC BY sounds play on the timeline; their credits go in the video's description. */
export function CreditsBar() {
  const count = useEditor((s) => {
    const project = s.snap?.project;
    if (!project) return 0;
    const used = new Set(project.tracks.flatMap((t) => t.clips.flatMap((c) => (c.content.type === "media" ? [c.content.assetId] : []))));
    return project.assets.filter((a) => a.credit?.license === "by" && used.has(a.id)).length;
  });
  if (count === 0) return null;
  return (
    <div className="flex shrink-0 items-center gap-2 border-t border-white/[.07] px-3.5 py-2.5 text-[12px] text-muted">
      <span className="min-w-0 flex-1">{count === 1 ? "1 sound needs credit" : `${count} sounds need credit`}</span>
      <Button pill className="h-7 gap-1.5 text-[12px]" onClick={copyCredits} title="Copy the credits for the video's description">
        <Copy size={13} /> Copy credits
      </Button>
    </div>
  );
}
