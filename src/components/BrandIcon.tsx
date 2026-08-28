/**
 * One brand mark, by name, at a size — the LobeHub set via `brand.ts`.
 *
 * Draws inline SVG so the mono form takes `currentColor` from wherever it sits
 * (a tinted badge, a selected tab) exactly like text would. The marks in the
 * hot tier render on the first pass; a lazy one renders as an empty box of the
 * same size and fills in when its chunk arrives, so nothing around it moves.
 *
 * A name the set does not know renders nothing at all — `brandForModel` and
 * friends already return null for those, and a generic placeholder would only
 * say "we could not identify this" in a spot meant to identify things.
 */
import { useEffect, useReducer } from "react";
import { hasBrand, loadSvg, preferredVariant, svgFor, type Variant } from "../brand";

export default function BrandIcon({
  name, size = 16, variant, className, title,
}: {
  name: string | null | undefined;
  size?: number;
  /** Force mono (for a mark inside a tinted control) or colour. Default: colour
   *  where the brand has one. */
  variant?: Variant;
  className?: string;
  title?: string;
}) {
  const n = name ?? "";
  const known = hasBrand(n);
  const v: Variant = known ? (variant ?? preferredVariant(n)) : "mono";
  const svg = known ? svgFor(n, v) : undefined;
  const [, bump] = useReducer((k: number) => k + 1, 0);
  useEffect(() => {
    if (!known || svg !== undefined) return;
    let live = true;
    loadSvg(n, v).then(() => { if (live) bump(); });
    return () => { live = false; };
  }, [known, n, v, svg]);
  if (!known) return null;
  return (
    <span
      className={"brand-icon " + n + (className ? " " + className : "")}
      style={{ fontSize: size }}
      title={title}
      aria-hidden={title ? undefined : true}
      dangerouslySetInnerHTML={svg !== undefined ? { __html: svg } : undefined}
    />
  );
}
