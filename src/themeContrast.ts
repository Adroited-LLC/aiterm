/** Contrast for opaque theme colours. Terminal ANSI colours stay untouched. */
function rgb(hex: string): number[] {
  return [1, 3, 5].map(i => parseInt(hex.slice(i, i + 2), 16));
}
function luminance(hex: string): number {
  return rgb(hex).reduce((sum, value, i) => {
    const c = value / 255;
    return sum + (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4) * [0.2126, 0.7152, 0.0722][i];
  }, 0);
}
export function contrastRatio(a: string, b: string): number {
  const x = luminance(a), y = luminance(b);
  return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05);
}
export function onAccent(accent: string): string {
  return contrastRatio(accent, "#ffffff") > contrastRatio(accent, "#16171d") ? "#ffffff" : "#16171d";
}
/** Lift secondary text just enough to remain readable on every panel surface.
 * Preserves each dark theme's hue and leaves already-readable colours alone. */
export function readableText(colour: string, surfaces: string[], minimum = 4.5): string {
  const passes = (candidate: string) => surfaces.every(bg => contrastRatio(candidate, bg) >= minimum);
  if (passes(colour)) return colour;
  const channels = rgb(colour);
  for (let step = 1; step <= 100; step++) {
    const candidate = "#" + channels.map(c => Math.round(c + (255 - c) * step / 100).toString(16).padStart(2, "0")).join("");
    if (passes(candidate)) return candidate;
  }
  return "#ffffff";
}
