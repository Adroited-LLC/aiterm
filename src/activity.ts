/**
 * Reading OpenRouter's activity record: who served this account, and at what
 * cost.
 *
 * The record's finest grain is one day of one model on one host, which is not
 * the question anyone asks of it. The question is "who has been serving me",
 * so the rows are folded to hosts, with the models underneath each one.
 *
 * Pure, and in its own module, so `npm run test:ui` can reach it — the
 * component that renders this has no test harness in this project, and the
 * arithmetic is the part worth being sure of.
 */

import type { ActivityRow } from "./ipc.ts";

/** One model's share of one host's traffic. */
export interface ModelUse {
  model: string;
  requests: number;
  /** Prompt plus completion — the record counts them apart, and nothing here
   *  asks the two questions separately. */
  tokens: number;
  /** USD. */
  usage: number;
}

/** Everything one host served this account over the window. */
export interface HostUse {
  /** The display name, exactly as OpenRouter gave it — "Baidu". */
  name: string;
  /** The same name normalised to routing form — "baidu" — which is what a
   *  policy's `resolved_ignore` is keyed by. */
  slug: string;
  requests: number;
  tokens: number;
  usage: number;
  models: ModelUse[];
}

/**
 * A display name in routing form.
 *
 * Activity reports hosts by name; the policy holds slugs. They are usually the
 * same word in different clothes, and assuming they are the same *string*
 * would silently fail to flag exactly the hosts this view exists to flag.
 *
 * The activity record names hosts in endpoint-tag form — `deepinfra/fp4`,
 * `google-vertex/global` — where everything after the slash is a variant of
 * the same company. The policy blocks the company, so the variant is cut
 * before slugifying: `deepinfra/fp4` must match a block on `deepinfra`, and
 * keeping the suffix would flag none of the tagged rows (seen in real data
 * 2026-08-10). Leading and trailing dashes are trimmed, matching Rust's
 * `slug()` — no OpenRouter slug has one, so a name that produced one could
 * only ever fail to match.
 */
export const slugOf = (name: string) =>
  name.split("/")[0].toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "");

/**
 * The daily rows folded to hosts, and to models within each host.
 *
 * Ordered by what was spent, then by how often — the biggest bill first is the
 * order someone reads this in, and free traffic still sorts by volume rather
 * than collapsing into one indistinguishable block at the bottom.
 */
export function groupActivity(rows: ActivityRow[]): HostUse[] {
  const hosts = new Map<string, HostUse & { byModel: Map<string, ModelUse> }>();
  for (const r of rows) {
    const tokens = r.prompt_tokens + r.completion_tokens;
    let h = hosts.get(r.provider_name);
    if (!h) {
      h = {
        name: r.provider_name,
        slug: slugOf(r.provider_name),
        requests: 0, tokens: 0, usage: 0,
        models: [],
        byModel: new Map(),
      };
      hosts.set(r.provider_name, h);
    }
    h.requests += r.requests;
    h.tokens += tokens;
    h.usage += r.usage;
    const m = h.byModel.get(r.model);
    if (m) {
      m.requests += r.requests;
      m.tokens += tokens;
      m.usage += r.usage;
    } else {
      h.byModel.set(r.model, {
        model: r.model, requests: r.requests, tokens, usage: r.usage,
      });
    }
  }
  const byCost = (a: ModelUse | HostUse, b: ModelUse | HostUse) =>
    b.usage - a.usage || b.requests - a.requests;
  return [...hosts.values()]
    .map(({ byModel, ...h }) => ({ ...h, models: [...byModel.values()].sort(byCost) }))
    .sort(byCost);
}

/** The account's whole window, for the line that says what this adds up to. */
export const totalOf = (hosts: HostUse[]) =>
  hosts.reduce(
    (t, h) => ({
      requests: t.requests + h.requests,
      tokens: t.tokens + h.tokens,
      usage: t.usage + h.usage,
    }),
    { requests: 0, tokens: 0, usage: 0 },
  );
