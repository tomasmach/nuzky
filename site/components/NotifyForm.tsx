"use client";

import { useEffect, useId, useRef, useState } from "react";
import { Check } from "lucide-react";

type State = "idle" | "sending" | "sent" | "invalid" | "failed";

const problems: Partial<Record<State, string>> = {
  invalid: "That address doesn't look right. Check it and try again.",
  failed: "That didn't go through. Try again in a minute.",
};

// Asks for an email address to send one message when the first version ships, and saves it through
// /api/notify. `noteId` points at the promise printed under the field, so a screen reader hears it with the field.
export function NotifyForm({ id, noteId }: { id?: string; noteId: string }) {
  const [state, setState] = useState<State>("idle");
  const field = useId();
  const problemId = useId();
  const thanks = useRef<HTMLParagraphElement>(null);
  const input = useRef<HTMLInputElement>(null);

  // After sending, the field is gone, so the thank-you takes the focus and gets read out. A refused address
  // sends the focus back to the field to fix it.
  useEffect(() => {
    if (state === "sent") thanks.current?.focus();
    if (state === "invalid") input.current?.focus();
  }, [state]);

  if (state === "sent") {
    return (
      <p
        ref={thanks}
        tabIndex={-1}
        role="status"
        className="glass flex h-12 w-full items-center justify-center gap-2 rounded-full px-5 text-[15px] font-medium outline-none sm:w-[400px]"
      >
        <Check className="size-4 text-ok" />
        Thanks. One email when Nuzky is out.
      </p>
    );
  }

  const problem = problems[state];
  const sending = state === "sending";

  return (
    <div className="flex w-full flex-col items-center gap-2 sm:w-[400px]">
      <form
        id={id}
        onSubmit={async (e) => {
          e.preventDefault();
          // The button stays enabled while sending, so the focus doesn't drop to the page; a second press waits.
          if (sending) return;
          const data = new FormData(e.currentTarget);
          setState("sending");
          try {
            const res = await fetch("/api/notify", {
              method: "POST",
              headers: { "Content-Type": "application/json" },
              body: JSON.stringify({ email: data.get("email"), website: data.get("website") }),
            });
            setState(res.ok ? "sent" : res.status === 400 ? "invalid" : "failed");
          } catch {
            setState("failed");
          }
        }}
        className="glass flex h-12 w-full scroll-mt-24 items-center gap-2 rounded-full p-1 pl-5 focus-within:ring-2 focus-within:ring-accent"
      >
        <label htmlFor={field} className="sr-only">
          Email
        </label>
        <input
          ref={input}
          id={field}
          name="email"
          type="email"
          required
          readOnly={sending}
          autoComplete="email"
          placeholder="you@example.com"
          aria-invalid={state === "invalid"}
          aria-describedby={problem ? `${problemId} ${noteId}` : noteId}
          className="min-w-0 flex-1 bg-transparent text-[15px] outline-none placeholder:text-muted"
        />
        {/* Bots fill in every field; people never see this one, so a value in it means a bot. */}
        <input name="website" tabIndex={-1} autoComplete="off" aria-hidden className="absolute -left-[9999px] size-px opacity-0" />
        <button
          type="submit"
          aria-disabled={sending}
          className="btn-light flex h-10 shrink-0 cursor-pointer items-center rounded-full px-[18px] text-[14px] font-semibold aria-disabled:cursor-default aria-disabled:opacity-60"
        >
          {sending ? "Sending…" : "Notify me"}
        </button>
      </form>
      {problem && (
        <p id={problemId} role="alert" className="text-[14px] text-warn">
          {problem}
        </p>
      )}
    </div>
  );
}
