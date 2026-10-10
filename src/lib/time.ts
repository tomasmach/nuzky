export const US = 1_000_000;

/** 00:12.40 style timecode; hours appear only when needed. */
export function formatTime(us: number, withFraction = true): string {
  const total = Math.max(0, us) / US;
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = Math.floor(total % 60);
  const cs = Math.floor((total * 100) % 100);
  const base = `${h > 0 ? `${h}:` : ""}${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}`;
  return withFraction ? `${base}.${String(cs).padStart(2, "0")}` : base;
}

/** Length of a video as people read it: 0:48, 12:05, 1:02:30. */
export function formatLength(us: number): string {
  const total = Math.round(Math.max(0, us) / US);
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = String(total % 60).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${s}` : `${m}:${s}`;
}

export function formatDuration(us: number): string {
  const s = us / US;
  return s < 60 ? `${s.toFixed(1)} s` : formatTime(us, false);
}

/** The time of day, with the date when it was not today. */
export function when(ms: number) {
  const at = new Date(ms);
  const time = at.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  return at.toDateString() === new Date().toDateString() ? time : `${at.toLocaleDateString([], { day: "numeric", month: "short" })} ${time}`;
}
