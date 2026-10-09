import { create } from "zustand";
import { api, errorText } from "./api";
import { useEditor } from "./store";

/**
 * Whether a newer CapOpen was released. A check asks the backend, which reads only the version on
 * the latest GitHub release. Automatic checks run 10 s after start and then at most once a day,
 * also across launches, and say nothing when they fail; Check for updates always answers.
 */
type Updates = {
  /** This CapOpen's version. */
  version: string;
  /** False when update checks are turned off for this installation; nothing is asked then. */
  enabled: boolean;
  /** Check by itself, as the user chose; on unless turned off. */
  auto: boolean;
  /** A newer released version, or null. */
  latest: string | null;
  /** A check in flight; one the user asked for shows until it answers. */
  checking: "auto" | "manual" | null;
};

type Saved = { auto: boolean; checkedAt: number; latest: string | null; /** The version that checked; another one checks anew. */ by: string };

const KEY = "capopen.updates";
const FIRST_CHECK_MS = 10_000;
const DAY_MS = 24 * 60 * 60 * 1000;
const HOUR_MS = 60 * 60 * 1000;

function load(): Partial<Saved> {
  try {
    const v = JSON.parse(localStorage.getItem(KEY) ?? "null");
    return v && typeof v === "object" ? v : {};
  } catch {
    return {};
  }
}

let checkedAt = 0;
let started = false;

export const useUpdates = create<Updates>(() => ({ version: "", enabled: false, auto: true, latest: null, checking: null }));

function save() {
  const { version, auto, latest } = useUpdates.getState();
  localStorage.setItem(KEY, JSON.stringify({ auto, checkedAt, latest, by: version } satisfies Saved));
}

/** Once per launch, after boot. */
export function startUpdates(version: string, enabled: boolean) {
  if (started) return;
  started = true;
  const saved = load();
  const same = saved.by === version;
  checkedAt = same && Number.isFinite(saved.checkedAt) ? Number(saved.checkedAt) : 0;
  useUpdates.setState({ version, enabled, auto: saved.auto !== false, latest: same && typeof saved.latest === "string" ? saved.latest : null });
  if (!enabled) return;
  window.setTimeout(checkIfDue, FIRST_CHECK_MS);
  window.setInterval(checkIfDue, HOUR_MS);
}

function checkIfDue() {
  if (useUpdates.getState().auto && Date.now() - checkedAt >= DAY_MS) void checkForUpdates(false);
}

/** `manual` answers with a toast, also when this is the latest version or the check failed, and also when it joins a check that was running. */
export async function checkForUpdates(manual: boolean) {
  const { enabled, checking, version } = useUpdates.getState();
  if (!enabled) return;
  // A check already on its way answers this one too.
  if (checking) {
    if (manual) useUpdates.setState({ checking: "manual" });
    return;
  }
  useUpdates.setState({ checking: manual ? "manual" : "auto" });
  const asked = () => useUpdates.getState().checking === "manual";
  // A failed check counts too, so an unreachable server is asked at most once a day.
  checkedAt = Date.now();
  try {
    const latest = await api.checkForUpdate();
    useUpdates.setState({ latest });
    if (asked()) {
      const toast = useEditor.getState().toast;
      if (latest) toast({ kind: "info", text: `CapOpen ${latest} is available.`, action: { label: "Download", run: downloadUpdate } });
      else toast({ kind: "success", text: `CapOpen ${version} is the latest version.` });
    }
  } catch (e) {
    console.warn("Update check failed", errorText(e));
    if (asked()) useEditor.getState().toast({ kind: "error", text: "Couldn't check for updates. Check your internet connection or try again later." });
  } finally {
    useUpdates.setState({ checking: null });
    save();
  }
}

export function setAutoCheck(auto: boolean) {
  useUpdates.setState({ auto });
  save();
  if (auto) checkIfDue();
}

/** Opens the latest release's page in the browser, where the installers are. */
export function downloadUpdate() {
  api.openReleasePage().catch((e) => useEditor.getState().toast({ kind: "error", text: `Couldn't open the download page: ${errorText(e)}` }));
}
