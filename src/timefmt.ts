/** How a moment is written wherever the UI says when something last happened.
 *
 *  `"relative"` is the usual "5m ago" / "3h" — fine for a glance, useless
 *  when three sessions all say "2h ago" and you need to know which was the
 *  one you were just in. `"absolute"` writes the clock time instead, and
 *  the day once it is not today.
 *
 *  A context rather than a prop: the ages are drawn in the sidebar, the
 *  home screen, the preview, the agent panel, and the composer pills, and
 *  threading one setting through all of those is how a value gets to a
 *  component by five different names. */
import { createContext, useContext } from "react";

export type TimeFormat = "relative" | "absolute";

export const TimeFormatContext = createContext<{
  format: TimeFormat;
  setFormat: (f: TimeFormat) => void;
}>({ format: "relative", setFormat: () => {} });

export function useTimeFormat() {
  return useContext(TimeFormatContext);
}

const DAY = 86_400_000;

function startOfToday(): number {
  const n = new Date();
  return new Date(n.getFullYear(), n.getMonth(), n.getDate()).getTime();
}

function clock(d: Date): string {
  return d.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
}

/** The full stamp, for tooltips: "Fri, Aug 28, 2026, 10:31 PM". */
export function fullTime(ms: number): string {
  return new Date(ms).toLocaleString([], {
    weekday: "short", month: "short", day: "numeric", year: "numeric",
    hour: "numeric", minute: "2-digit",
  });
}

/** Clock time for today, the weekday within the week, the date beyond it. */
export function absTime(ms: number): string {
  const d = new Date(ms);
  const today = startOfToday();
  if (ms >= today) return clock(d);
  if (ms >= today - 6 * DAY) return `${d.toLocaleDateString([], { weekday: "short" })} ${clock(d)}`;
  if (d.getFullYear() === new Date().getFullYear()) {
    return `${d.toLocaleDateString([], { month: "short", day: "numeric" })}, ${clock(d)}`;
  }
  return d.toLocaleDateString([], { month: "short", day: "numeric", year: "numeric" });
}

/** The same, squeezed for a row corner: "10:31p", "Tue 10:31p", "Aug 21", "Jul 2025". */
export function absTimeShort(ms: number): string {
  const d = new Date(ms);
  const today = startOfToday();
  const c = clock(d).replace(/\s?AM$/i, "a").replace(/\s?PM$/i, "p");
  if (ms >= today) return c;
  if (ms >= today - 6 * DAY) return `${d.toLocaleDateString([], { weekday: "short" })} ${c}`;
  if (d.getFullYear() === new Date().getFullYear()) {
    return d.toLocaleDateString([], { month: "short", day: "numeric" });
  }
  return d.toLocaleDateString([], { month: "short", year: "numeric" });
}

/** "5m ago" / "3h ago" / "2d ago". */
export function relTime(ms: number): string {
  const s = Math.floor((Date.now() - ms) / 1000);
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  return `${Math.floor(s / 86400)}d ago`;
}

/** Compact relative time for a row corner: "now", "5m", "3h", "2d", "3mo". */
export function relTimeShort(ms: number): string {
  const s = Math.floor((Date.now() - ms) / 1000);
  if (s < 60) return "now";
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h`;
  if (s < 30 * 86400) return `${Math.floor(s / 86400)}d`;
  return `${Math.floor(s / (30 * 86400))}mo`;
}

export function fmtTime(ms: number, format: TimeFormat): string {
  return format === "absolute" ? absTime(ms) : relTime(ms);
}

export function fmtTimeShort(ms: number, format: TimeFormat): string {
  return format === "absolute" ? absTimeShort(ms) : relTimeShort(ms);
}
