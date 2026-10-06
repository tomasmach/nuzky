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

export function formatDuration(us: number): string {
  const s = us / US;
  return s < 60 ? `${s.toFixed(1)} s` : formatTime(us, false);
}
