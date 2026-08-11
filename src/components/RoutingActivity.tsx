import { useEffect, useState } from "react";
import { ActivityRow, ProviderView, providerActivity } from "../ipc";
import { groupActivity, totalOf } from "../activity";

/** USD. Zero is a real answer here — free models are most of why this view is
 *  worth reading — so it is printed as `$0.00` rather than "free", and a real
 *  but sub-cent bill is marked as such instead of rounding down into it. */
const fmtUsd = (n: number) =>
  n === 0 ? "$0.00" : n < 0.01 ? "<$0.01" : `$${n.toFixed(2)}`;

/** 1234 → "1.2K", 3_400_000 → "3.4M". Exact counts are noise at this scale. */
const fmtTok = (n: number) =>
  n >= 1_000_000 ? `${+(n / 1_000_000).toFixed(1)}M`
    : n >= 1_000 ? `${+(n / 1_000).toFixed(1)}K`
      : String(n);

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

/**
 * What actually ran: every host that served this account, what it cost, and
 * which of them the current policy would now refuse.
 *
 * That last column is the report the routing feature exists to produce. A
 * policy says what will happen next; this says what already happened — and a
 * host flagged here is one that served traffic before the rule that now blocks
 * it. Activity names hosts as it displays them and the policy holds routing
 * slugs, so the two are matched on a normalised name (see `activity.ts`);
 * comparing the strings as given would flag nothing and look like good news.
 *
 * The numbers are OpenRouter's, for the whole account — see the note under the
 * table. Nothing here is aiterm's own accounting, and it must not read as if
 * it were.
 */
export default function RoutingActivity({ prov }: { prov: ProviderView }) {
  const [rows, setRows] = useState<ActivityRow[] | null>(null);
  const [err, setErr] = useState<string | null>(null);
  /** The host whose models are unfolded. One at a time: the interesting
   *  question is per host, and every model of every host at once is a wall. */
  const [open, setOpen] = useState<string | null>(null);
  /** Bumped by Refresh. Also the way a key that is stored but unchanged in
   *  shape — two short keys swapped — gets a second chance. */
  const [reload, setReload] = useState(0);

  // Re-asks when the management key changes, because the last answer was very
  // likely OpenRouter's refusal rather than a record.
  const keyStamp = `${prov.has_management_key}:${prov.management_key_hint}`;
  useEffect(() => {
    let dropped = false;
    setRows(null); setErr(null); setOpen(null);
    providerActivity(prov.id)
      // A reply that lands after the provider has changed belongs to the one
      // being left — the cleanup runs before the next effect, so it is dropped
      // rather than written under someone else's name.
      .then((r) => { if (!dropped) setRows(r); })
      .catch((e) => { if (!dropped) setErr(String(e)); });
    return () => { dropped = true; };
  }, [prov.id, keyStamp, reload]);

  const hosts = rows ? groupActivity(rows) : [];
  const total = totalOf(hosts);
  const blocked = prov.policy.resolved_ignore;

  return (
    <div className="acty-body">
      <div className="set-hint">
        OpenCode sessions appear here only — aiterm renders its screen but never
        reads its stream.
      </div>

      {err && <div className="set-notice">{err}</div>}
      {!err && rows === null && (
        <div className="set-hint mb-wait">Asking OpenRouter what ran…</div>
      )}
      {rows !== null && hosts.length === 0 && (
        <div className="set-hint mb-wait">Nothing in OpenRouter's record for this key.</div>
      )}

      {hosts.map((h) => {
        const why = blocked[h.slug];
        return (
          <div key={h.name} className="acty-host">
            <button
              className={"acty-row" + (open === h.name ? " on" : "")}
              title={h.models.length === 1 && open !== h.name
                ? h.models[0].model
                : `${plural(h.models.length, "model")} — click to ${open === h.name ? "fold" : "list"}`}
              onClick={() => setOpen(open === h.name ? null : h.name)}
            >
              <span className="acty-name">{h.name || "Unnamed host"}</span>
              {why && (
                <span className="acty-blocked" title={`Excluded: ${why}`}>now blocked</span>
              )}
              <span className="acty-n">{plural(h.requests, "request")}</span>
              <span className="acty-tok">{fmtTok(h.tokens)} tok</span>
              <span className="acty-usd">{fmtUsd(h.usage)}</span>
            </button>
            {open === h.name && (
              <div className="acty-models">
                {h.models.map((m) => (
                  <div key={m.model} className="acty-row acty-model">
                    <span className="acty-name">{m.model || "unnamed model"}</span>
                    <span className="acty-n">{plural(m.requests, "request")}</span>
                    <span className="acty-tok">{fmtTok(m.tokens)} tok</span>
                    <span className="acty-usd">{fmtUsd(m.usage)}</span>
                  </div>
                ))}
              </div>
            )}
          </div>
        );
      })}

      {hosts.length > 0 && (
        <div className="acty-total">
          {plural(hosts.length, "host")} · {plural(total.requests, "request")} ·
          {" "}{fmtTok(total.tokens)} tok · {fmtUsd(total.usage)}
        </div>
      )}

      <div className="acty-acts">
        <button className="act-btn" onClick={() => setReload((n) => n + 1)}>Refresh</button>
      </div>

      {/* Account-wide, not app-wide. Letting it read as aiterm's own spend
          would be a lie the panel tells by omission. */}
      <div className="set-hint">
        OpenRouter's record for this key — including requests aiterm did not make.
      </div>
    </div>
  );
}
