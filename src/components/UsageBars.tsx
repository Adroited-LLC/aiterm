import { useEffect, useState } from "react";
import { UsageBar, usageLimits } from "../ipc";

/** Relative "resets in 3h 55m" from an ISO timestamp; "" if unknown/past. */
function resetsIn(iso: string): string {
  if (!iso) return "";
  const ms = new Date(iso).getTime() - Date.now();
  if (!isFinite(ms) || ms <= 0) return "resetting";
  const mins = Math.round(ms / 60000);
  const d = Math.floor(mins / 1440);
  const h = Math.floor((mins % 1440) / 60);
  const m = mins % 60;
  if (d > 0) return `resets in ${d}d ${h}h`;
  if (h > 0) return `resets in ${h}h ${m}m`;
  return `resets in ${m}m`;
}

/**
 * Three compact plan-usage bars mirroring claude.ai's usage view (session /
 * weekly-all / weekly-scoped). No text inline — hovering a bar shows a popup
 * with that limit's label, percent, and reset time. Colour comes from the
 * API severity. Hidden entirely when there's no data (offline / not signed in).
 */
export function UsageBars() {
  const [bars, setBars] = useState<UsageBar[]>([]);
  const [hover, setHover] = useState<number | null>(null);

  useEffect(() => {
    let alive = true;
    // Keep the last good reading. `usage_limits` returns [] on any transient
    // failure (curl hiccup, token mid-refresh); replacing bars with [] on those
    // makes them blink out for up to a poll interval — the "flaky" behaviour.
    // Only replace when we actually got data.
    const load = () =>
      usageLimits()
        .then((b) => alive && b.length && setBars(b))
        .catch(() => {});
    load();
    const iv = setInterval(load, 60_000);
    return () => {
      alive = false;
      clearInterval(iv);
    };
  }, []);

  if (!bars.length) return null;

  return (
    <div className="usage-bars" title="">
      {bars.map((b, i) => (
        <div
          key={b.kind + i}
          className="usage-bar"
          onMouseEnter={() => setHover(i)}
          onMouseLeave={() => setHover((h) => (h === i ? null : h))}
        >
          <div className="usage-track">
            <div
              className={"usage-fill sev-" + b.severity}
              style={{ width: Math.min(100, Math.max(0, b.percent)) + "%" }}
            />
          </div>
          {hover === i && (
            <div className="usage-tip">
              <span className="usage-tip-label">{b.label}</span>
              <span className="usage-tip-pct">{Math.round(b.percent)}% used</span>
              {resetsIn(b.resets_at) && (
                <span className="usage-tip-reset">{resetsIn(b.resets_at)}</span>
              )}
            </div>
          )}
        </div>
      ))}
    </div>
  );
}
