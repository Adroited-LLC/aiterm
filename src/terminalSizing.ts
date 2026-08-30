export interface TerminalGridSize {
  cols: number;
  rows: number;
}

export type TerminalFocus = "desktop" | "remote" | "unowned";

/** Decide which grid xterm may display without diverging from Rust.
 *
 * Only the desktop focus owner may turn a local FitAddon measurement into an
 * authoritative resize. Every other desktop projection must keep xterm on the
 * dimensions broadcast by the registry, even when its container could fit a
 * different grid.
 */
export function projectTerminalGrid(
  focus: TerminalFocus,
  canonical: TerminalGridSize | undefined,
  fitted: TerminalGridSize,
): { size: TerminalGridSize; resizeBackend: boolean } {
  if (focus === "desktop") return { size: fitted, resizeBackend: true };
  return { size: canonical ?? fitted, resizeBackend: false };
}
