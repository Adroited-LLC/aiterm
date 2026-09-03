/**
 * How much of each service is left, on the home screen.
 *
 * The top bar's strip already carries these numbers, and it stays exactly as
 * it is — but the strip is a glance you take while working, four chips two
 * millimetres tall in the corner of a 1920px window. Home is the one screen
 * where there is room to say it properly, and "how much Claude have I got
 * left before I start something" is a question you ask *here*, before opening
 * anything, not halfway through a turn.
 *
 * So: one row per service, with its mark, a bar, the worst percentage as a
 * number, and when it resets if the service says. Nothing is fetched — this
 * renders the same `usageSources` App already polls for the strip, so the two
 * can never disagree and there is no second request.
 */
import { UsageSourceAt, headline, resetsIn } from "./UsagePanel";
import BrandIcon from "./BrandIcon";
import { brandForUsageSource } from "../brand";

/** The window closest to a wall — the one that will stop you. */
function worstBar(src: UsageSourceAt) {
  if (!src.bars.length) return null;
  return src.bars.reduce((a, b) => (b.percent > a.percent ? b : a));
}

export default function HomeUsage({ sources }: { sources: UsageSourceAt[] }) {
  // A service with neither a window nor a balance has nothing to draw, and an
  // empty track beside a name would read as "you have used none of it".
  const rows = sources.filter((s) => s.bars.length > 0 || s.amounts.length > 0);
  if (rows.length === 0) return null;

  return (
    <section className="home-usage">
      <h2 className="fleet-head">Usage</h2>
      <div className="home-usage-rows">
        {rows.map((s) => {
          const bar = worstBar(s);
          const head = headline(s);
          const reset = bar ? resetsIn(bar.resets_at) : "";
          const pct = bar ? Math.min(100, Math.max(0, bar.percent)) : null;
          return (
            <div className={"home-usage-row" + (s.stale ? " stale" : "")} key={s.id}>
              <span className="home-usage-name" title={s.plan ? `${s.name} — ${s.plan}` : s.name}>
                <BrandIcon name={brandForUsageSource(s.id, s.name)} size={14} className="inline" />
                {s.name}
              </span>
              <span className="home-usage-track">
                {pct !== null && (
                  <span className={"home-usage-fill sev-" + bar!.severity} style={{ width: pct + "%" }} />
                )}
              </span>
              <span className={"home-usage-n sev-" + head.severity}>{head.text}</span>
              <span className="home-usage-reset">{reset}</span>
            </div>
          );
        })}
      </div>
    </section>
  );
}
