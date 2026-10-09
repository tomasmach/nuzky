// `sizes` for an image inside a Fit composition: `w` CSS pixels at the design width, scaled down with the
// composition once the page (minus its side padding) is narrower than the design.
export const fitSizes = (w: number, design: number, padding = 40) =>
  `(min-width: ${design + padding}px) ${w}px, ${Math.ceil((w / design) * 100)}vw`;
