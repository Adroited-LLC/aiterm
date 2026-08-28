/**
 * One UI icon from Lucide (https://lucide.dev, ISC), sized by the Appearance
 * setting rather than by the call site.
 *
 * `--icon-size` is set from `settings.iconSize`; `md` is that, `sm` a step
 * under it for row actions and inline marks, `lg` a step over. A caller that
 * must be a fixed size (the engine badge, which shares a box with the brand
 * marks) passes `px` instead. Stroke is a touch lighter than Lucide's default
 * two, which reads heavy at 16px beside 12–13px text.
 *
 * Brand marks are not these — see `BrandIcon`.
 */
import type { CSSProperties } from "react";
import type { LucideIcon, LucideProps } from "lucide-react";

export default function Icon({
  of: Glyph, size = "md", px, className, style, ...rest
}: {
  of: LucideIcon;
  size?: "sm" | "md" | "lg";
  px?: number;
} & Omit<LucideProps, "size" | "ref">) {
  const fixed: CSSProperties | undefined = px ? { width: px, height: px } : undefined;
  return (
    <Glyph
      className={"ui-icon " + size + (className ? " " + className : "")}
      strokeWidth={1.75}
      aria-hidden
      style={fixed ? { ...fixed, ...style } : style}
      {...rest}
    />
  );
}
