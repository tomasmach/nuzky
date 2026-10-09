"use client";

import { useLayoutEffect, useRef, useState } from "react";

// Lays a fixed-size composition out at its design size and scales it down to the available width,
// so the reels and the editor keep their exact proportions on every screen.
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
  const [scale, setScale] = useState(1);

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const ro = new ResizeObserver(([entry]) => setScale(Math.min(1, entry.contentRect.width / width)));
    ro.observe(el);
    return () => ro.disconnect();
  }, [width]);

  return (
    <div ref={ref} data-waves={waves} className={`relative w-full ${className}`} style={{ maxWidth: width, aspectRatio: `${width} / ${height}` }}>
      <div className="absolute left-0 top-0 origin-top-left" style={{ width, height, transform: `scale(${scale})` }}>
        {children}
      </div>
    </div>
  );
}
