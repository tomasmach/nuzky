/**
 * Follows a pointer drag on the window until the button is released. When the system takes the
 * pointer (pointercancel) or the window loses focus, the drag ends through `cancel` instead, so no
 * drag stays stuck waiting for a release that never comes. Returns a function that stops following.
 */
export function followPointer(on: { move?: (e: PointerEvent) => void; up?: (e: PointerEvent) => void; cancel?: () => void }): () => void {
  const move = (e: PointerEvent) => on.move?.(e);
  const up = (e: PointerEvent) => {
    stop();
    on.up?.(e);
  };
  const cancel = () => {
    stop();
    on.cancel?.();
  };
  const stop = () => {
    window.removeEventListener("pointermove", move);
    window.removeEventListener("pointerup", up);
    window.removeEventListener("pointercancel", cancel);
    window.removeEventListener("blur", cancel);
  };
  window.addEventListener("pointermove", move);
  window.addEventListener("pointerup", up);
  window.addEventListener("pointercancel", cancel);
  window.addEventListener("blur", cancel);
  return stop;
}
