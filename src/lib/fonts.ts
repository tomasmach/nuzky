import { useEffect, useState } from "react";
import { api, errorText } from "./api";
import type { FontFamilies } from "./types";
import manifest from "../../assets/fonts/manifest.json";

/** What the engine draws when a text has no font set. */
export const DEFAULT_FONT = "Inter";

// The engine embeds the same files from the same manifest, so pickers and swatches show the real face.
const URLS = import.meta.glob<string>("../../assets/fonts/**/*.ttf", { query: "?url", import: "default", eager: true });
const FACES = manifest.map(({ family, file, weight }) => ({ family, weight, url: URLS[`../../assets/fonts/${file}`] }));
const BUNDLED = new Set(FACES.map((face) => face.family));

// Registered under a prefix so the bundled faces never replace the UI font. The browser
// only downloads a face once something is drawn with it.
const cssName = (family: string) => `CapOpen ${family}`;
if (typeof document !== "undefined" && "fonts" in document) {
  for (const { family, url, weight } of FACES) document.fonts.add(new FontFace(cssName(family), `url("${url}")`, { weight }));
}

/** CSS font-family for previewing `family`; installed families are used by name. */
export function fontCss(family: string | null | undefined): string {
  const f = family || DEFAULT_FONT;
  return BUNDLED.has(f) ? `"${cssName(f)}", sans-serif` : `"${f.replace(/"/g, "")}", sans-serif`;
}

let request: Promise<FontFamilies> | null = null;

type FontList = { fonts: FontFamilies | null; error: string | null };

/** Families the engine can draw, fetched once per session. */
export function useFontFamilies(): FontList {
  const [state, setState] = useState<FontList>({ fonts: null, error: null });
  useEffect(() => {
    let live = true;
    request ??= api.listFonts();
    request.then(
      (fonts) => live && setState({ fonts, error: null }),
      (e) => {
        request = null;
        if (live) setState({ fonts: null, error: errorText(e) });
      },
    );
    return () => {
      live = false;
    };
  }, []);
  return state;
}
