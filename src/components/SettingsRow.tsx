import { ReactNode } from "react";

/** One setting: name and an optional one-line description on the left, the
 *  control on the right. Every pane is built from these, so every setting
 *  reads the same way.
 *
 *  Lives here rather than in SettingsModal because panes that own their own
 *  controls — the renderer sample, which has to sit above its buttons — need
 *  the same row shape, and copying the markup would let the two drift. */
export default function Row({ label, desc, children, wide }: {
  label: string;
  desc?: string;
  children?: ReactNode;
  /** Control needs the full width (theme grid, preview) — render it under the text. */
  wide?: boolean;
}) {
  return (
    <div className={"srow" + (wide ? " wide" : "")}>
      <div className="srow-info">
        <div className="srow-label">{label}</div>
        {desc && <div className="srow-desc">{desc}</div>}
      </div>
      {children && <div className="srow-ctl">{children}</div>}
    </div>
  );
}
