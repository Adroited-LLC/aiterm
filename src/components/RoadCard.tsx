import { useState, type ReactNode } from "react";
import type { RoadState } from "./remoteRoads.ts";

/**
 * One way a phone reaches this desktop: a switch, a name, one plain sentence,
 * a dot that says whether it is working, and a "Settings" disclosure that
 * opens that road's own controls inline. Every road looks the same so the
 * list reads as a list.
 */
export default function RoadCard({ name, desc, state, disabled, onToggle, children }: {
  name: string;
  desc: string;
  state: RoadState;
  disabled?: boolean;
  onToggle: (on: boolean) => void;
  /** The road's settings; omitted when there is nothing to set. */
  children?: ReactNode;
}) {
  const [open, setOpen] = useState(false);
  return (
    <div className={"road" + (state.on ? " on" : "")}>
      <div className="road-head">
        <label className="sw" aria-label={name}>
          <input
            type="checkbox"
            checked={state.on}
            disabled={disabled}
            onChange={(e) => onToggle(e.target.checked)}
          />
          <span className="sw-track"><span className="sw-knob" /></span>
        </label>
        <div className="road-text">
          <div className="road-name">{name}</div>
          <div className="srow-desc">{desc}</div>
          <div className="road-status">
            <span className={"road-dot " + state.dot} aria-hidden="true" />
            <span className="road-status-text">{state.text}</span>
          </div>
          {state.lines?.map((line) => (
            <div key={line} className="road-status road-sub">{line}</div>
          ))}
        </div>
        {children && (
          <button
            type="button"
            className={"act-btn road-disclose" + (open ? " on" : "")}
            aria-expanded={open}
            onClick={() => setOpen((v) => !v)}
          >Settings</button>
        )}
      </div>
      {children && open && <div className="road-body">{children}</div>}
    </div>
  );
}
