import { type RefObject, useEffect, useState } from "react";

// A demo plays only while its section is on screen, the tab is visible and the visitor has not
// asked for reduced motion. All three are followed live, so a demo stops in a hidden tab and the
// moment reduced motion is switched on.
export function usePlaying(ref: RefObject<Element | null>, threshold = 0) {
  const [playing, setPlaying] = useState(false);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const still = window.matchMedia("(prefers-reduced-motion: reduce)");
    let onScreen = false;
    const update = () => setPlaying(onScreen && !document.hidden && !still.matches);
    const io = new IntersectionObserver(
      ([e]) => {
        onScreen = e.isIntersecting;
        update();
      },
      { threshold },
    );
    io.observe(el);
    document.addEventListener("visibilitychange", update);
    still.addEventListener("change", update);
    return () => {
      io.disconnect();
      document.removeEventListener("visibilitychange", update);
      still.removeEventListener("change", update);
    };
  }, [ref, threshold]);
  return playing;
}
