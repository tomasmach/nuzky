import { useEffect, useState } from "react";
import { api, errorText } from "./api";
import type { FontFamilies } from "./types";
// The same files the engine embeds (crates/engine text.rs), so pickers and swatches show the real face.
import anton from "../../assets/fonts/anton/Anton-Regular.ttf?url";
import bebas from "../../assets/fonts/bebasneue/BebasNeue-Regular.ttf?url";
import inter from "../../assets/fonts/inter/Inter[opsz,wght].ttf?url";
import lexend from "../../assets/fonts/lexend/Lexend[wght].ttf?url";
import montserrat from "../../assets/fonts/montserrat/Montserrat[wght].ttf?url";
import oswald from "../../assets/fonts/oswald/Oswald[wght].ttf?url";
import poppinsBold from "../../assets/fonts/poppins/Poppins-Bold.ttf?url";
import poppins from "../../assets/fonts/poppins/Poppins-Regular.ttf?url";
import roboto from "../../assets/fonts/roboto/Roboto[wdth,wght].ttf?url";

/** What the engine draws when a text has no font set. */
export const DEFAULT_FONT = "Inter";

const FACES: [family: string, url: string, weight: string][] = [
  ["Anton", anton, "400"],
  ["Bebas Neue", bebas, "400"],
  ["Inter", inter, "100 900"],
  ["Lexend", lexend, "100 900"],
  ["Montserrat", montserrat, "100 900"],
  ["Oswald", oswald, "200 700"],
  ["Poppins", poppins, "400"],
  ["Poppins", poppinsBold, "700"],
  ["Roboto", roboto, "100 900"],
];
const BUNDLED = new Set(FACES.map(([family]) => family));

// Registered under a prefix so the bundled faces never replace the UI font. The browser
// only downloads a face once something is drawn with it.
const cssName = (family: string) => `CapOpen ${family}`;
if (typeof document !== "undefined" && "fonts" in document) {
  for (const [family, url, weight] of FACES) document.fonts.add(new FontFace(cssName(family), `url("${url}")`, { weight }));
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
