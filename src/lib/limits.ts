import type { Limits } from "./types";

/** The engine's editing limits, set from the boot payload before the editor renders. */
export let LIMITS: Limits;

export function setLimits(limits: Limits) {
  LIMITS = limits;
}
