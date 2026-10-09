"use client";

import { useEffect, useRef } from "react";

// Lays a fixed-size composition out at its design size and scales it down to the available width,
// so the reels and the editor keep their exact proportions on every screen.
//
// The scale is the `--fit` variable. An inline script sets it while the HTML is still parsing, so a phone never
// paints the composition at full size while the JavaScript loads (or when it fails to run), and a resize only
// updates the variable instead of rendering the whole composition again.
export function Fit({
  width,
  height,
  className = "",
  waves,
  children,
}: {
  width: number;
  height: number;
  className?: string;
  /** Marks the composition as an anchor for the light behind it (see Waves). */
  waves?: "swell" | "glow";
  children: React.ReactNode;
}) {
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const ro = new ResizeObserver(([entry]) => el.style.setProperty("--fit", String(Math.min(1, entry.contentRect.width / width))));
    ro.observe(el);
    return () => ro.disconnect();
  }, [width]);

  return (
    // The inline script adds `--fit` to the style before React hydrates, so the attribute differs on purpose.
    <div
      ref={ref}
      data-waves={waves}
      suppressHydrationWarning
      className={`relative w-full ${className}`}
      style={{ maxWidth: width, aspectRatio: `${width} / ${height}` }}
    >
      <script
        dangerouslySetInnerHTML={{
          __html: `(function(e){e.style.setProperty("--fit",Math.min(1,e.clientWidth/${width}))})(document.currentScript.parentElement)`,
        }}
      />
      <div className="absolute left-0 top-0 origin-top-left" style={{ width, height, transform: "scale(var(--fit, 1))" }}>
        {children}
      </div>
    </div>
  );
}
