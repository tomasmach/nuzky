"use client";

import { useEffect, useId, useRef, useState } from "react";
import { Check } from "lucide-react";

// Asks for an email address to send one message when the first version ships. `noteId` points at the
// promise printed under the field, so a screen reader hears it with the field.
// Not wired yet: a valid address only shows the thank-you and goes nowhere until an email service is picked.
export function NotifyForm({ id, noteId }: { id?: string; noteId: string }) {
  const [sent, setSent] = useState(false);
  const field = useId();
  const thanks = useRef<HTMLParagraphElement>(null);

  // The field the focus was on is gone, so the thank-you takes the focus and gets read out.
  useEffect(() => {
    if (sent) thanks.current?.focus();
  }, [sent]);

  if (sent) {
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

  return (
    <form
      id={id}
      onSubmit={(e) => {
        e.preventDefault();
        setSent(true);
      }}
      className="glass flex h-12 w-full scroll-mt-24 items-center gap-2 rounded-full p-1 pl-5 focus-within:ring-2 focus-within:ring-accent sm:w-[400px]"
    >
      <label htmlFor={field} className="sr-only">
        Email
      </label>
      <input
        id={field}
        name="email"
        type="email"
        required
        autoComplete="email"
        placeholder="you@example.com"
        aria-describedby={noteId}
        className="min-w-0 flex-1 bg-transparent text-[15px] outline-none placeholder:text-muted"
      />
      <button type="submit" className="btn-light flex h-10 shrink-0 cursor-pointer items-center rounded-full px-[18px] text-[14px] font-semibold">
        Notify me
      </button>
    </form>
  );
}
