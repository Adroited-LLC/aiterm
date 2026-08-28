import { useEffect, useRef, useState } from "react";
import { UsageAmount, UsageBar, UsageSource, relTime } from "../ipc";
import BrandIcon from "./BrandIcon";
import { brandForUsageSource } from "../brand";

/**
 * How much of everything you have left, for every service aiterm can see.
 *
 * ## Why this is not three little bars any more
 *
 * It used to be: a 42px-wide, textless bar per Anthropic limit, wedged into
 * the top bar, with the label and the percentage only in a hover tooltip. You
 * could not read your usage — you could only find out that it existed and then
 * go hunting for a number with the mouse. Three anonymous slivers also gave
 * nowhere to put Codex, and no shape at all for a credit balance, which is a
 * quantity of money rather than a fraction of a window.
 *
 * So there are two views over the same reading:
 *
 * * a **strip** in the top bar, one chip per service, each carrying its name
 *   and its worst number *as text*, with a hairline under it in the severity
 *   colour. Legible without touching the mouse.
 * * a **panel**, opened by clicking the strip, which is the whole reading:
 *   every window with its label, percent, full-width bar and reset time; every
 *   balance; the signed-in account and plan; and, for a service that could not
 *   be reached, a sentence saying which of the several different problems that
 *   is.
 *
 * A popover rather than a docked panel because usage is something you check,
 * not something you keep open next to your work — and the right-hand column is
 * already three resizable panels deep. It stays open until dismissed (click
 * away, or Escape), so unlike the tooltip it does not vanish when you move the
 * pointer towards the thing you were reading.
 *
 * ## The reading is owned by App
 *
 * Passed in, never fetched here. `/api/oauth/usage` rate limits; one poller
 * feeds this, the composer's usage pill, and anything else that grows a view.
 */

/** A source plus when its numbers were actually read. */
export interface UsageSourceAt extends UsageSource {
  /** Epoch ms of the reading these numbers came from. */
  at: number;
  /** True when this is a kept reading and the latest attempt failed —
   *  `detail` then says why the refresh failed, not why the numbers are gone. */
  stale: boolean;
  /** The state of that failed attempt ("limited", "unreachable", …) when
   *  `stale`; "" otherwise. What kind of failure decides whether the strip
   *  flags it. */
  failed: string;
}

/**
 * Fold a fresh report into the one on screen, keeping the last good numbers
 * for anything that just failed.
 *
 * This is the rule the backend cannot implement, because it is stateless per
 * call: a source coming back `unreachable` means "could not ask", and blanking
 * its bars would render as "you have used nothing" — the one reading it must
 * never be mistaken for. So the old numbers stay, marked stale and stamped
 * with when they were read, and the failure becomes a line of explanation
 * rather than an absence.
 *
 * Sources absent from `next` are dropped: that is a provider you deleted, and
 * keeping its balance around would be worse than forgetting it.
 */
export function mergeUsage(
  prev: UsageSourceAt[],
  next: UsageSource[],
  now = Date.now(),
): UsageSourceAt[] {
  const before = new Map(prev.map((p) => [p.id, p]));
  return next.map((s) => {
    if (s.state === "ok") return { ...s, at: now, stale: false, failed: "" };
    const old = before.get(s.id);
    if (old && old.state === "ok") {
      return { ...old, stale: true, failed: s.state, detail: s.detail || old.detail };
    }
    return { ...s, at: now, stale: false, failed: "" };
  });
}

/** Relative "resets in 3h 55m" from an ISO timestamp; "" if unknown/past. */
function resetsIn(iso: string): string {
  if (!iso) return "";
  const ms = new Date(iso).getTime() - Date.now();
  if (!isFinite(ms)) return "";
  if (ms <= 0) return "resetting";
  const mins = Math.round(ms / 60000);
  const d = Math.floor(mins / 1440);
  const h = Math.floor((mins % 1440) / 60);
  const m = mins % 60;
  if (d > 0) return `resets in ${d}d ${h}h`;
  if (h > 0) return `resets in ${h}h ${m}m`;
  return `resets in ${m}m`;
}

/**
 * A money-ish number, with a currency symbol only where the backend is sure of
 * the currency — Anthropic names it and OpenRouter's credits are dollars, so
 * those get a `$`; Codex and Grok balances name nothing and print bare rather
 * than acquiring a symbol this app made up.
 *
 * Four decimals under 1 so a balance of 93 cents does not round to nothing,
 * which is exactly the case where the number matters most.
 */
export function fmtAmount(n: number, currency: string): string {
  const abs = Math.abs(n);
  const body = n.toFixed(abs > 0 && abs < 1 ? 4 : 2);
  if (currency === "USD") return `$${body}`;
  return currency ? `${body} ${currency}` : body;
}

function amountLine(a: UsageAmount): string {
  const value = fmtAmount(a.amount, a.currency);
  if (a.of === null) return value;
  const total = fmtAmount(a.of, a.currency);
  return a.sense === "used" ? `${value} of ${total} used` : `${value} left of ${total}`;
}

/** The one number a chip shows, and the colour it shows it in. */
function headline(s: UsageSource): { text: string; severity: string } {
  if (s.bars.length) {
    // The worst window is the one that will stop you, so it is the one that
    // belongs in a single-number summary.
    const worst = s.bars.reduce((a, b) => (b.percent > a.percent ? b : a));
    return { text: `${Math.round(worst.percent)}%`, severity: worst.severity };
  }
  if (s.amounts.length) {
    const a = s.amounts[0];
    return { text: fmtAmount(a.amount, a.currency), severity: "normal" };
  }
  return { text: "—", severity: "none" };
}

/** Worst wins, so the service closest to a wall colours the strip. */
const SEVERITIES = ["none", "normal", "warning", "critical"];

/**
 * The strip's colour comes only from real percentages, never from a source
 * that failed. A provider that publishes no balance is not a warning, and
 * painting the whole strip amber because one of four services was briefly
 * unreachable would teach you to ignore the colour — which is the only thing
 * it is for. A source in an unknown state gets a "?" chip instead.
 */
function stripSeverity(sources: UsageSourceAt[]): string {
  let worst = 0;
  for (const s of sources) {
    if (s.state !== "ok") continue;
    for (const b of s.bars) worst = Math.max(worst, SEVERITIES.indexOf(b.severity));
  }
  return SEVERITIES[Math.max(0, worst)];
}

function BarRow({ bar }: { bar: UsageBar }) {
  const reset = resetsIn(bar.resets_at);
  return (
    <div className="usage-row">
      <div className="usage-row-head">
        <span className="usage-row-label">{bar.label}</span>
        <span className="usage-row-value">{Math.round(bar.percent)}%</span>
      </div>
      <div className="usage-track">
        <div
          className={"usage-fill sev-" + bar.severity}
          style={{ width: Math.min(100, Math.max(0, bar.percent)) + "%" }}
        />
      </div>
      {reset && <div className="usage-row-sub">{reset}</div>}
    </div>
  );
}

function AmountRow({ amount }: { amount: UsageAmount }) {
  // A capped amount gets a bar too; a bare balance has nothing to be a
  // fraction of, and drawing an empty track next to it would imply one.
  const pct =
    amount.of && amount.of > 0
      ? Math.min(100, Math.max(0, (amount.sense === "used"
          ? amount.amount / amount.of
          : 1 - amount.amount / amount.of) * 100))
      : null;
  return (
    <div className="usage-row">
      <div className="usage-row-head">
        <span className="usage-row-label">{amount.label}</span>
        <span className="usage-row-value">{amountLine(amount)}</span>
      </div>
      {pct !== null && (
        <div className="usage-track">
          <div
            className={"usage-fill sev-" + (pct >= 90 ? "critical" : pct >= 75 ? "warning" : "normal")}
            style={{ width: pct + "%" }}
          />
        </div>
      )}
    </div>
  );
}

function SourceCard({ src }: { src: UsageSourceAt }) {
  const broken = src.state !== "ok";
  const stale = src.stale;
  return (
    <div className="usage-src">
      <div className="usage-src-head">
        <span className="usage-src-name">
          <BrandIcon name={brandForUsageSource(src.id, src.name)} size={14} className="inline" />
          {src.name}
        </span>
        {src.plan && <span className="usage-src-plan">{src.plan}</span>}
        <span className="usage-src-spacer" />
        {src.account && <span className="usage-src-acct" title={src.account}>{src.account}</span>}
      </div>
      {/* A failure and a stale reading say different things. "Couldn't reach
          chatgpt.com" with no numbers means you know nothing; the same sentence
          under numbers from four minutes ago means you know something slightly
          old. Both are worth saying, and saying them the same way would make
          the second look like the first. */}
      {broken && !stale && <div className={"usage-src-detail " + src.state}>{src.detail}</div>}
      {src.bars.map((b, i) => (
        <BarRow key={b.kind + i} bar={b} />
      ))}
      {src.amounts.map((a, i) => (
        <AmountRow key={a.label + i} amount={a} />
      ))}
      {src.notes.map((n, i) => (
        <div key={i} className="usage-src-note">{n}</div>
      ))}
      {stale && (
        <div className="usage-src-detail stale">
          Last read {relTime(src.at)}
          {src.detail ? ` — ${src.detail}` : ", refreshing…"}
        </div>
      )}
    </div>
  );
}

interface Props {
  sources: UsageSourceAt[];
  /** Ask for a fresh reading now. The poller keeps running either way; this is
   *  the same one poller, told to go early. */
  onRefresh: () => void;
  refreshing: boolean;
}

export function UsagePanel({ sources, onRefresh, refreshing }: Props) {
  // Two ways open. Hovering the strip peeks at the panel and leaving closes
  // it, which is how a glance at "what is my weekly at" wants to work; a
  // click pins it, so it survives the pointer wandering off to read, until
  // dismissed. Both feed one `open`, and the panel is inside the hovered
  // element, so moving the pointer down into it keeps the peek alive.
  const [pinned, setPinned] = useState(false);
  const [hover, setHover] = useState(false);
  const open = pinned || hover;
  const ref = useRef<HTMLDivElement>(null);
  const hoverTimer = useRef<number | null>(null);
  const armHover = (on: boolean) => {
    if (hoverTimer.current !== null) window.clearTimeout(hoverTimer.current);
    // A short wait in: a pointer crossing the top bar should not flash the
    // panel. A short grace out: the gap between the strip and the panel
    // must not close it on the way there.
    hoverTimer.current = window.setTimeout(() => setHover(on), on ? 180 : 260);
  };
  useEffect(() => () => {
    if (hoverTimer.current !== null) window.clearTimeout(hoverTimer.current);
  }, []);
  const setOpen = (v: boolean | ((o: boolean) => boolean)) => {
    const next = typeof v === "function" ? v(open) : v;
    setPinned(next);
    if (!next) setHover(false);
  };

  // Dismiss on an outside click or Escape — a popover you have to click twice
  // to close is worse than the tooltip it replaced.
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  // Nothing at all has been read yet — not even a failure. Show a quiet
  // placeholder rather than nothing, so the strip does not appear out of
  // thin air a minute into the session.
  const chips = sources.length
    ? sources
    : [{
        id: "pending", name: "Usage", state: "ok", detail: "", plan: "", account: "",
        bars: [], amounts: [], notes: [], at: 0, stale: false, failed: "",
      } as UsageSourceAt];

  // Four chips: one per agent CLI (Claude, Codex, Grok) plus the API balance,
  // which is the whole point of the strip — a fourth service folded into a
  // "+1" is a service you cannot read. Past that they are a count, and the
  // panel has all of them.
  const shown = chips.length > 4 ? chips.slice(0, 4) : chips;
  const hidden = chips.length - shown.length;

  // Sources whose numbers we do not currently know. "no_balance" is not one of
  // them — most OpenAI-compatible endpoints simply do not publish a balance,
  // and flagging that would cry wolf. Neither is a cache freshly loaded from
  // disk, which is stale but carries no failure to report. Nor is a rate
  // limit ("limited"): the service will answer the next poll, the number on
  // the chip is a minute old at most, and a mark that lit every time
  // Anthropic said "not right now" was a mark you learned to ignore.
  const routine = (state: string) => state === "ok" || state === "no_balance" || state === "limited";
  const blind = sources.filter(
    (s) => !routine(s.state) || (s.stale && !!s.detail && !routine(s.failed)),
  );
  const newest = sources.reduce((a, s) => Math.max(a, s.at), 0);
  const providers = sources.filter((s) => s.id.startsWith("provider:")).length;

  return (
    <div
      className="usage"
      ref={ref}
      onPointerEnter={() => armHover(true)}
      onPointerLeave={() => armHover(false)}
    >
      <button
        className={"usage-strip sev-" + stripSeverity(sources) + (open ? " on" : "")}
        title="Plan limits and credit balances"
        onClick={() => setOpen((o) => !o)}
      >
        {shown.map((s) => {
          const h = s.state === "ok" ? headline(s) : { text: "—", severity: "none" };
          return (
            <span key={s.id} className={"usage-chip sev-" + h.severity}>
              <span className="usage-chip-name">
                <BrandIcon name={brandForUsageSource(s.id, s.name)} size={12} className="inline" />
                {s.name}
              </span>
              <span className="usage-chip-value">{h.text}</span>
              <span
                className="usage-chip-rule"
                style={{
                  // The rule is the old bar, kept because a shape you can read
                  // at a glance is worth 3px — but it is under a number now,
                  // not instead of one.
                  width:
                    s.bars.length
                      ? Math.min(100, Math.max(2, Math.max(...s.bars.map((b) => b.percent)))) + "%"
                      : "0%",
                }}
              />
            </span>
          );
        })}
        {hidden > 0 && <span className="usage-chip more">+{hidden}</span>}
        {blind.length > 0 && (
          // Named, one per line: "one service could not be read" sends you
          // into the panel to find out which, which is the question the mark
          // exists to answer. A stale source says when it was last read too,
          // since its number is still on the chip and is that old.
          <span
            className="usage-chip unknown"
            title={blind
              .map((s) =>
                s.stale
                  ? `${s.name}: last read ${relTime(s.at)} — ${s.detail}`
                  : `${s.name}: ${s.detail}`,
              )
              .join("\n")}
          >?</span>
        )}
      </button>
      {open && (
        <div className="usage-panel">
          <div className="usage-panel-head">
            <span>USAGE</span>
            <div>
              {/* Disabled while a read is in flight, which is the only
                  throttle it needs: a leaned-on button would just collect
                  429s from Anthropic, and a 429 keeps the previous numbers
                  and says so, so the worst case is a stale line rather than
                  a blank panel. */}
              <button
                className="icon-btn"
                title="Read it again now"
                disabled={refreshing}
                onClick={onRefresh}
              >⟳</button>
              <button className="icon-btn" title="Close" onClick={() => setOpen(false)}>✕</button>
            </div>
          </div>
          <div className="usage-panel-body">
            {sources.length === 0 ? (
              <div className="empty-note">
                Nothing read yet — the first reading lands within a minute of opening.
              </div>
            ) : (
              sources.map((s) => <SourceCard key={s.id} src={s} />)
            )}
          </div>
          <div className="usage-panel-foot">
            {newest ? `Read ${relTime(newest)}. Refreshes every minute.` : "Reading…"}
            {providers === 0 && " No API providers configured."}
          </div>
        </div>
      )}
    </div>
  );
}
