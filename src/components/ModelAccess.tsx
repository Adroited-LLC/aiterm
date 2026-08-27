import { useEffect, useRef, useState } from "react";
import {
  DirectoryEntry, EndpointCard, MaxPrice, ModelCard, Policy, ProviderView,
  providerDelete, providerDirectory, providerManagementKeySet, providerModelCards,
  providerModelEndpoints, providerModels, providerPolicySet, providerRouteSet,
  providerSave, providerStartupSet, providersList, Route,
} from "../ipc";
import RoutingActivity from "./RoutingActivity";

/** Base URLs worth not making people look up. Anything OpenAI-compatible works;
 *  these are just the ones most people are reaching for. */
const PRESETS: { name: string; base_url: string }[] = [
  { name: "OpenRouter", base_url: "https://openrouter.ai/api/v1" },
  { name: "OpenAI", base_url: "https://api.openai.com/v1" },
  { name: "Groq", base_url: "https://api.groq.com/openai/v1" },
  { name: "Together", base_url: "https://api.together.xyz/v1" },
  { name: "Local (llama.cpp / vLLM)", base_url: "http://localhost:8080/v1" },
];

/** 200000 → "200K", 1048576 → "1M". Dash when the provider didn't say. */
const fmtCtx = (n: number | null) =>
  n == null ? "—" : n >= 1_000_000 ? `${+(n / 1_000_000).toFixed(1)}M` : `${Math.round(n / 1000)}K`;

/** Per-token USD → "$3/M" (or "free"). Dash when the provider didn't say. */
const fmtPrice = (p: number | null) =>
  p == null ? "—" : p === 0 ? "free" : `$${+(p * 1e6).toFixed(2)}/M`;

/** Free means the provider *said* zero — both directions, not just unknown. */
const isFree = (m: ModelCard) =>
  m.prompt_price === 0 && (m.completion_price ?? 0) === 0;

const DAY = 86_400;

/** Epoch seconds → "today" or "N days ago". Days is the only unit worth
 *  showing here: what it dates is a directory read, and the thing it warns
 *  about — hosts added since — happens on that scale. */
const ago = (secs: number) => {
  const d = Math.floor((Date.now() / 1000 - secs) / DAY);
  return d <= 0 ? "today" : d === 1 ? "1 day ago" : `${d} days ago`;
};

/** A compiled ignore list stops describing the directory it was built from. 30
 *  days is where we say so out loud rather than let the rule look stronger
 *  than it is. */
const STALE_AFTER = 30 * DAY;

/** One ceiling field of the policy draft. Text, not a number, because it is
 *  being typed in; `bad` is the number field's own verdict on input it could
 *  not parse, which reaches us as `""` and would otherwise read as "no cap". */
type CapField = { text: string; bad: boolean };

/** The policy being edited. Saving compiles it against the live directory, so
 *  it is held here until the button is pressed rather than written per
 *  keystroke. Countries are held uppercase; the backend matches
 *  case-insensitively, so this only keeps the toggles from doubling up. */
type PolDraft = {
  countries: string[];
  unknown: boolean;
  /** Slugs blocked by name. Removable here; nothing adds to it yet. */
  providers: string[];
  prompt: CapField;
  completion: CapField;
};

const draftOf = (pol: Policy): PolDraft => ({
  // Both lists are de-duplicated on the way in: they are keys in the render,
  // and a hand-edited providers.json is under no obligation to be tidy.
  countries: [...new Set(pol.blocked_countries.map((c) => c.toUpperCase()))],
  unknown: pol.block_unknown_country,
  providers: [...new Set(pol.blocked_providers)],
  prompt: { text: pol.max_price.prompt == null ? "" : String(pol.max_price.prompt), bad: false },
  completion: {
    text: pol.max_price.completion == null ? "" : String(pol.max_price.completion),
    bad: false,
  },
});

/** A draft ceiling as a number, `undefined` for "no cap", or `"bad"` for text
 *  that is not either — which fails the save rather than silently clearing. */
const capValue = (f: CapField): number | undefined | "bad" => {
  if (f.bad) return "bad";
  const t = f.text.trim();
  if (t === "") return undefined;
  const n = Number(t);
  return Number.isFinite(n) && n >= 0 ? n : "bad";
};

/**
 * Where API model access is configured.
 *
 * Deliberately honest about its own state: nothing runs a session against these
 * yet, and the panel says so rather than implying otherwise. What it does do is
 * real — **Test** calls the provider's `/models` and reports what came back, so
 * a wrong key or a typo'd URL is caught here instead of much later.
 *
 * The key is write-only from this side. It is sent on save and never returned;
 * a saved provider shows only whether it has one and the last four characters.
 * Leaving the field blank when editing keeps the stored key, so changing a URL
 * does not mean digging the secret out again.
 */
export default function ModelAccess() {
  const [providers, setProviders] = useState<ProviderView[] | null>(null);
  const [editing, setEditing] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [baseUrl, setBaseUrl] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  /** Per-provider Test result: the models found, or why not. */
  const [tested, setTested] = useState<Record<string, string>>({});
  const [confirmDel, setConfirmDel] = useState<string | null>(null);

  // The model browser: which provider it is open on, the cards each provider
  // returned (kept per id, so reopening is instant), and the view state.
  const [browsing, setBrowsing] = useState<string | null>(null);
  const [cards, setCards] = useState<Record<string, ModelCard[]>>({});
  const [browseErr, setBrowseErr] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<"all" | "free" | "paid" | "starred">("all");
  const [minCtx, setMinCtx] = useState(0);
  const [picked, setPicked] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  /** Providers that have answered a real request this session — the green
   *  dot. Either button proves it; Test and Models hit the same endpoint. */
  const [healthy, setHealthy] = useState<Record<string, boolean>>({});

  // Who hosts each model, kept per model id: the card follows arrow-key
  // selection through 400 rows, and fetching on selection would be one request
  // per keystroke. So it is fetched when the section is opened, and cached.
  const [endpoints, setEndpoints] = useState<Record<string, EndpointCard[]>>({});
  const [epOpen, setEpOpen] = useState<string | null>(null);
  /** Scoped to a model, so an error on one card does not follow the selection
   *  onto the next one. */
  const [epErr, setEpErr] = useState<{ model: string; msg: string } | null>(null);
  /** The ceiling field being typed in, as text. One at a time, keyed
   *  `<model>:<side>`, so the stored value shows everywhere else and a
   *  half-typed number is never sent. `bad` carries the number field's own
   *  verdict on the text it could not parse, which reaches us as `""` and
   *  would otherwise read as a deliberate clear. */
  const [cap, setCap] = useState<{ key: string; text: string; bad: boolean } | null>(null);

  // The account-wide policy. It is one section per provider, so all of this is
  // cleared when the browser moves — see `browse`.
  const [polOpen, setPolOpen] = useState(false);
  /** Every host OpenRouter knows, fetched when the section is opened. It is
   *  where the country toggles and the unknown-country count come from. */
  const [dir, setDir] = useState<DirectoryEntry[] | null>(null);
  const [draft, setDraft] = useState<PolDraft | null>(null);
  /** The slug being typed into the block field. Held apart from the draft: it
   *  is not part of the policy until Block puts it there. */
  const [blockSlug, setBlockSlug] = useState("");
  const [polErr, setPolErr] = useState<string | null>(null);
  const [polBusy, setPolBusy] = useState(false);

  // What actually ran. Its own section, next to the policy it reports against:
  // one says what will happen, the other what already did.
  const [actOpen, setActOpen] = useState(false);
  /** The management key being typed in. `/activity` is the only endpoint that
   *  refuses an inference key, so this is the only place that asks for one. */
  const [mgmtKey, setMgmtKey] = useState("");
  const [mgmtBusy, setMgmtBusy] = useState(false);
  const [mgmtErr, setMgmtErr] = useState<string | null>(null);
  /** The newest provider list, including one a route write has just returned
   *  but React has not re-rendered from yet. */
  const provsRef = useRef<ProviderView[] | null>(null);
  /** The provider the browser is open on *now*. A request that is in flight
   *  captured the one it was sent for; the two differ when the user switches
   *  providers before it lands, and the model-keyed caches below are cleared
   *  on that switch — so a late row must be dropped, not written. */
  const browsingRef = useRef<string | null>(null);
  /** One tick per policy save, and one per move of the model browser. A save
   *  captures it before its first await and tests it after each one: a newer
   *  save, or any move of the browser, makes the captured value stale and the
   *  settling save writes nothing. It is stronger than comparing the provider
   *  id, which reads equal again when the user leaves a provider and comes
   *  back — so `savePolicy` gates on this alone. */
  const polGen = useRef(0);
  /** Route writes run one at a time — see `applyRoute`. */
  const routeQ = useRef<Promise<void>>(Promise.resolve());

  const browse = async (p: ProviderView) => {
    // Every move of the browser, fold included, retires the saves in flight:
    // the policy section they write into belongs to the provider being left.
    polGen.current += 1;
    if (browsing === p.id) {
      setBrowsing(null); // second click folds it away
      browsingRef.current = null;
      return;
    }
    setBrowsing(p.id);
    browsingRef.current = p.id;   // in flight already; the effect is too late
    setBrowseErr(null);
    setPicked(null);
    // Endpoints are keyed by model id alone, so a second OpenRouter account
    // would otherwise be shown the first one's rows.
    setEndpoints({}); setEpOpen(null); setEpErr(null); setCap(null);
    // Likewise the policy: the draft, the directory and the section's own
    // open/closed state all belong to the provider being left. `polBusy` is
    // among them — a save still settling on the provider we are leaving must
    // not leave this one's Save button reading "Saving…", and the tick above
    // stops that save from clearing the flag a later save has set.
    setPolOpen(false); setDir(null); setDraft(null); setPolErr(null); setPolBusy(false);
    setBlockSlug("");
    // And the activity section: the record is per account, and a half-typed
    // management key belongs to the provider it was being typed for.
    setActOpen(false); setMgmtKey(""); setMgmtErr(null); setMgmtBusy(false);
    if (!cards[p.id]) {
      try {
        const list = await providerModelCards(p.id);
        setCards((c) => ({ ...c, [p.id]: list }));
        setHealthy((h) => ({ ...h, [p.id]: true }));
      } catch (e) {
        setBrowseErr(String(e));
        setHealthy((h) => ({ ...h, [p.id]: false }));
      }
    }
  };

  /** The provider whose models are open, as currently saved. */
  const browsingProv = providers?.find((x) => x.id === browsing);

  const toggleStartup = async (modelId: string) => {
    if (!browsingProv) return;
    const cur = browsingProv.startup_models;
    const next = cur.includes(modelId)
      ? cur.filter((m) => m !== modelId)
      : [...cur, modelId];
    try {
      setProviders(await providerStartupSet(browsingProv.id, next));
    } catch (e) {
      setBrowseErr(String(e));
    }
  };

  /** Routing is an OpenRouter feature, and the backend refuses it on anything
   *  else — the same test, so the section is not offered where it would fail. */
  const isOpenRouter = !!browsingProv?.base_url.includes("openrouter.ai");

  const openEndpoints = async (modelId: string) => {
    if (epOpen === modelId) { setEpOpen(null); return; }
    setEpOpen(modelId); setEpErr(null);
    if (!endpoints[modelId] && browsingProv) {
      const provId = browsingProv.id;
      try {
        const rows = await providerModelEndpoints(provId, modelId);
        if (browsingRef.current !== provId) return;   // switched away mid-flight
        setEndpoints((e) => ({ ...e, [modelId]: rows }));
      } catch (e) {
        if (browsingRef.current !== provId) return;
        setEpErr({ model: modelId, msg: String(e) });
      }
    }
  };

  /**
   * Store one model's route, built from the freshest copy of it.
   *
   * Every write carries the fields it is not changing forward, so a pin and a
   * ceiling committed inside one round trip would have the second undo the
   * first. Two guards: the writes run one at a time, and each one reads the
   * route it is amending from the list the previous write returned rather than
   * from the render that scheduled it.
   */
  const applyRoute = (modelId: string, build: (cur: Route | undefined) => Route) => {
    const provId = browsing;
    if (!provId) return routeQ.current;
    setEpErr(null);
    // Whether the rows are on screen is the caller's question, not the
    // queue's: it is what the user is looking at as they act.
    const showing = !!endpoints[modelId];
    routeQ.current = routeQ.current.then(async () => {
      const prov = provsRef.current?.find((x) => x.id === provId);
      if (!prov) return;
      try {
        const list = await providerRouteSet(provId, modelId, build(prov.routes[modelId]));
        provsRef.current = list;
        setProviders(list);
        // The rows carry `excluded`, which the new pin or ceiling may change.
        if (showing) {
          const rows = await providerModelEndpoints(provId, modelId);
          if (browsingRef.current !== provId) return;   // switched away mid-flight
          setEndpoints((e) => ({ ...e, [modelId]: rows }));
        }
      } catch (e) {
        if (browsingRef.current !== provId) return;
        setEpErr({ model: modelId, msg: String(e) });
      }
    });
    return routeQ.current;
  };

  /** `null` unpins. An empty slug is neither: a row whose tag named no
   *  provider parses to `slug: ""`, and storing that would write an order of
   *  one nameless host — a pin that can only fail. Clicking such a row does
   *  nothing rather than something wrong. */
  const pin = (modelId: string, slug: string | null) => {
    if (slug !== null && !slug) return routeQ.current;
    return applyRoute(modelId, (cur) => ({
      order: slug ? [slug] : [],
      allow_fallbacks: false,      // a pin means only that host
      max_price: cur?.max_price ?? {},
    }));
  };

  const capKey = (modelId: string, side: keyof MaxPrice) => `${modelId}:${side}`;

  /** The saved ceiling as field text. */
  const capStored = (modelId: string, side: keyof MaxPrice) => {
    const v = browsingProv?.routes[modelId]?.max_price[side];
    return v == null ? "" : String(v);
  };

  /** What the field shows: the draft while it is being typed in, the stored
   *  ceiling otherwise. */
  const capText = (modelId: string, side: keyof MaxPrice) => {
    const k = capKey(modelId, side);
    if (cap?.key === k) return cap.text;
    return capStored(modelId, side);
  };

  /** What an empty field would fall back to. A per-model ceiling *replaces*
   *  the account one rather than merging, so once this model sets either side
   *  the other side inherits nothing. */
  const capHint = (modelId: string, side: keyof MaxPrice) => {
    const own = browsingProv?.routes[modelId]?.max_price;
    if (own && (own.prompt != null || own.completion != null)) return "no cap";
    const acct = browsingProv?.policy.max_price[side];
    return acct == null ? "no cap" : String(acct);
  };

  /** Blur or Enter, never per keystroke: each commit is a write to disk. */
  const commitCap = async (modelId: string, side: keyof MaxPrice, el?: HTMLInputElement) => {
    const k = capKey(modelId, side);
    if (cap?.key !== k) return;             // nothing was typed here
    const text = cap.text.trim();
    const bad = cap.bad;
    setCap(null);
    if (!browsingProv) return;
    // Garbage reverts to what is stored rather than clearing it. A number
    // field hands us `""` for text it cannot parse, so only `badInput` tells
    // `1e` from a field the user emptied on purpose. Clearing the draft does
    // not put the stored text back on screen either — the bound value was
    // `""` before and after, so React writes nothing — hence the box itself.
    if (bad) { if (el) el.value = capStored(modelId, side); return; }
    const n = text === "" ? undefined : Number(text);
    if (n !== undefined && (!Number.isFinite(n) || n < 0)) return;
    // Every good commit writes, even one that matches what is on screen. What
    // it would be compared against is not settled until the write queued ahead
    // of it lands, so a "no change" test here can only read a stale route —
    // and retyping the old value inside one round trip would then keep the
    // newer one. The last commit must win; a redundant write is the price.
    await applyRoute(modelId, (cur) => {
      const max_price: MaxPrice = { ...(cur?.max_price ?? {}) };
      if (n === undefined) delete max_price[side]; else max_price[side] = n;
      return { order: cur?.order ?? [], allow_fallbacks: false, max_price };
    });
  };

  /**
   * Open the policy section, seeding the draft and fetching the directory.
   *
   * The draft survives a fold — collapsing the section is not a decision to
   * throw away what was typed into it — so it is seeded only when there is
   * none. Switching provider clears it, which is a different thing.
   */
  const openPolicy = async () => {
    if (polOpen) { setPolOpen(false); return; }
    const p = browsingProv;
    if (!p) return;
    setPolOpen(true);
    setPolErr(null);
    if (!draft) setDraft(draftOf(p.policy));
    if (!dir) {
      const provId = p.id;
      try {
        const rows = await providerDirectory(provId);
        if (browsingRef.current !== provId) return;   // switched away mid-flight
        setDir(rows);
      } catch (e) {
        if (browsingRef.current !== provId) return;
        setPolErr(String(e));
      }
    }
  };

  const editDraft = (f: (d: PolDraft) => PolDraft) => setDraft((d) => (d ? f(d) : d));

  /** Block one host by name. Slugs are lowercase in the directory and a
   *  repeat would be a second chip removing the first, so the field is
   *  trimmed, lowercased and checked against what is already listed. */
  const addBlocked = () => {
    const s = blockSlug.trim().toLowerCase();
    if (!s) return;
    editDraft((d) => (d.providers.includes(s)
      ? d
      : { ...d, providers: [...d.providers, s] }));
    setBlockSlug("");
  };

  const toggleCountry = (c: string) =>
    editDraft((d) => ({
      ...d,
      countries: d.countries.includes(c)
        ? d.countries.filter((x) => x !== c)
        : [...d.countries, c],
    }));

  /**
   * Compile and store the policy.
   *
   * `resolved_ignore` and `resolved_at` are sent empty: the backend recomputes
   * both against the directory it reads at save time, and a directory that
   * will not load fails the save rather than storing a rule that blocks
   * nothing.
   */
  const savePolicy = async () => {
    const p = browsingProv;
    if (!p || !draft || polBusy) return;
    // Claim a tick before anything else. Two clicks in one tick both read
    // `polBusy` false, and a fold-then-reopen resets it while the first save
    // is still in flight — in both the second save now retires the first.
    const myGen = (polGen.current += 1);
    const prompt = capValue(draft.prompt);
    const completion = capValue(draft.completion);
    if (prompt === "bad" || completion === "bad") {
      setPolErr("A ceiling must be a number of dollars per million tokens, or blank for none.");
      return;
    }
    const max_price: MaxPrice = {};
    if (prompt !== undefined) max_price.prompt = prompt;
    if (completion !== undefined) max_price.completion = completion;
    setPolBusy(true); setPolErr(null);
    const provId = p.id;
    try {
      const list = await providerPolicySet(provId, {
        blocked_countries: draft.countries,
        block_unknown_country: draft.unknown,
        blocked_providers: draft.providers,
        max_price,
        resolved_ignore: {},
        resolved_at: 0,
      });
      // The provider list is whole-account state, not the browser's view of
      // one provider, so it is stored whichever provider is on screen — the
      // same as `applyRoute`. Everything below it is view state, and guarded.
      provsRef.current = list;
      setProviders(list);
      // Every endpoint row carries `excluded`, computed from the stored
      // policy — so the cached rows now describe the rule that was just
      // replaced. Drop them, and re-fetch the one set that is on screen,
      // because nothing else would.
      const open = epOpen;
      // The map is keyed by model id alone, so once the browser has moved on
      // it holds the *other* provider's rows — which this policy did not
      // touch. Wiping them there would blank a list that is on screen.
      if (polGen.current !== myGen) return;
      setEndpoints({});
      if (open) {
        try {
          const rows = await providerModelEndpoints(provId, open);
          // Merged, not assigned: another section opened while this was in
          // flight has already written its rows, and replacing the map would
          // strand it on "Asking the provider…" with no request left to land.
          if (polGen.current === myGen) setEndpoints((e) => ({ ...e, [open]: rows }));
        } catch (e) {
          if (polGen.current === myGen) setEpErr({ model: open, msg: String(e) });
        }
      }
    } catch (e) {
      // Errors are shown inside the policy section, which now belongs to
      // another provider — a failed save on the one we left must not print
      // there.
      if (polGen.current === myGen) setPolErr(String(e));
    } finally {
      // Gated like the rest, and for two reasons: `browse` already cleared
      // this flag for the provider now on screen, and a save that started
      // after this one owns the flag now. Clearing it from here would free the
      // Save button while another save is still in flight. Folding the browser
      // away leaves the flag set, which is harmless — the section is not
      // rendered, and reopening the provider runs the reset in `browse`.
      if (polGen.current === myGen) setPolBusy(false);
    }
  };

  /**
   * Store or clear the management key `/activity` wants.
   *
   * Blank is not "keep the stored one" here, unlike the provider form: Save is
   * disabled on an empty field and Forget sends the empty string on purpose,
   * so neither action can happen by way of the other.
   *
   * Guarded on the provider it was sent for, like every other write in this
   * panel — the field and its error line belong to one provider's section.
   */
  const saveMgmtKey = async (key: string) => {
    const p = browsingProv;
    if (!p || mgmtBusy) return;
    const provId = p.id;
    setMgmtBusy(true); setMgmtErr(null);
    try {
      // Whole-account state, so it is stored whichever provider is on screen —
      // the same as `applyRoute` and `savePolicy`.
      const list = await providerManagementKeySet(provId, key);
      provsRef.current = list;
      setProviders(list);
      if (browsingRef.current === provId) setMgmtKey("");
    } catch (e) {
      if (browsingRef.current === provId) setMgmtErr(String(e));
    } finally {
      if (browsingRef.current === provId) setMgmtBusy(false);
    }
  };

  const pol = browsingProv?.policy;
  const ignored = pol ? Object.keys(pol.resolved_ignore).length : 0;
  const stale = !!pol && pol.resolved_at > 0
    && Date.now() / 1000 - pol.resolved_at > STALE_AFTER;

  /** The countries a rule can name: every one the directory mentions, plus any
   *  already blocked — a host leaving the directory must not quietly take the
   *  toggle for its country with it. CN leads because it is the one this was
   *  built for, and it is offered even if the directory somehow omits it. */
  const countries = (() => {
    const set = new Set<string>(["CN"]);
    for (const e of dir ?? []) {
      if (e.headquarters) set.add(e.headquarters.toUpperCase());
      for (const c of e.datacenters) set.add(c.toUpperCase());
    }
    for (const c of draft?.countries ?? []) set.add(c);
    return ["CN", ...[...set].filter((c) => c !== "CN").sort()];
  })();

  /** How many hosts name this country at all — headquarters or a datacenter,
   *  which is exactly what the compile matches on. */
  const countryCount = (c: string) =>
    (dir ?? []).filter(
      (e) => e.headquarters?.toUpperCase() === c
        || e.datacenters.some((d) => d.toUpperCase() === c),
    ).length;

  /** Hosts that report no country at all — neither headquarters nor a single
   *  datacenter. Too large a share to decide silently in either direction,
   *  hence its own checkbox with the count beside it. */
  const unknownCount = (dir ?? []).filter(
    (e) => !e.headquarters && e.datacenters.length === 0,
  ).length;

  const all = browsing ? cards[browsing] : undefined;
  const q = query.trim().toLowerCase();
  const starred = browsingProv?.startup_models ?? [];
  const shown = (all ?? []).filter(
    (m) =>
      (!q || m.id.toLowerCase().includes(q) || (m.name ?? "").toLowerCase().includes(q)) &&
      (filter === "all" ||
        (filter === "starred" ? starred.includes(m.id) : (filter === "free") === isFree(m))) &&
      (minCtx === 0 || (m.context_length ?? 0) >= minCtx),
  );
  // The card follows the pick, or the first match — so it always shows
  // something while there is anything to show.
  const sel = shown.find((m) => m.id === picked) ?? shown[0];

  const copyId = (id: string) => {
    navigator.clipboard?.writeText(id);
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1500);
  };

  const refresh = () => providersList().then(setProviders).catch(() => setProviders([]));
  useEffect(() => { refresh(); }, []);
  useEffect(() => { provsRef.current = providers; }, [providers]);
  useEffect(() => { browsingRef.current = browsing; }, [browsing]);

  const reset = () => {
    setEditing(null); setName(""); setBaseUrl(""); setApiKey(""); setError(null);
  };

  const submit = async () => {
    setBusy(true); setError(null);
    try {
      setProviders(await providerSave(editing, name, baseUrl, apiKey));
      reset();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const test = async (p: ProviderView) => {
    setTested((t) => ({ ...t, [p.id]: "Asking…" }));
    try {
      const models = await providerModels(p.id);
      // The count is the useful part; a couple of names make it obvious you
      // reached the provider you meant rather than some other one.
      setTested((t) => ({
        ...t,
        [p.id]: `${models.length} models — ${models.slice(0, 3).join(", ")}${models.length > 3 ? "…" : ""}`,
      }));
      setHealthy((h) => ({ ...h, [p.id]: true }));
    } catch (e) {
      setTested((t) => ({ ...t, [p.id]: String(e) }));
      setHealthy((h) => ({ ...h, [p.id]: false }));
    }
  };

  const edit = (p: ProviderView) => {
    setEditing(p.id);
    setName(p.name);
    setBaseUrl(p.base_url);
    setApiKey("");            // never prefilled — it is not readable
    setError(null);
  };

  return (
    <div className="set-section">
      {/* No heading of its own — the settings pane title already says where
          we are. */}
      <div className="set-hint">
        Any OpenAI-compatible endpoint. Keys are stored in
        {" "}<code>~/.config/aiterm/providers.json</code> with 0600 permissions, and are
        never shown again after saving.
      </div>

      {providers === null ? (
        <div className="set-hint">Loading…</div>
      ) : (
        <div className="prov-list">
          {providers.map((p) => (
            <div key={p.id} className="prov-row">
              <div className="prov-text">
                <div className="prov-name">
                  {healthy[p.id] && (
                    <span className="prov-dot" title="Answered a model list this session" />
                  )}
                  {p.name}
                  <span className="prov-key">
                    {p.has_key ? (p.key_hint ? `key …${p.key_hint}` : "key saved") : "no key"}
                  </span>
                </div>
                <div className="prov-url">{p.base_url}</div>
                {tested[p.id] && <div className="prov-test">{tested[p.id]}</div>}
              </div>
              <div className="prov-acts">
                <button
                  className={"act-btn" + (browsing === p.id ? " on" : "")}
                  title="Browse its models"
                  onClick={() => browse(p)}
                >Models</button>
                <button className="act-btn" title="Ask for its model list" onClick={() => test(p)}>Test</button>
                <button className="act-btn" onClick={() => edit(p)}>Edit</button>
                {confirmDel === p.id ? (
                  <>
                    <button
                      className="act-btn danger"
                      onClick={async () => {
                        setConfirmDel(null);
                        setProviders(await providerDelete(p.id));
                      }}
                    >Remove</button>
                    <button className="act-btn" onClick={() => setConfirmDel(null)}>Cancel</button>
                  </>
                ) : (
                  <button className="act-btn danger" onClick={() => setConfirmDel(p.id)}>✕</button>
                )}
              </div>
            </div>
          ))}
          {providers.length === 0 && (
            <div className="set-hint">Nothing configured yet.</div>
          )}
        </div>
      )}

      {browsing && (
        <div className="mb">
          {/* Account-wide, so it sits above the model list rather than in a
              model's card: one rule, applied to every request this provider
              serves. */}
          {isOpenRouter && pol && (
            <div className="pol">
              <div className="pol-head">
                <button
                  className={"act-btn" + (polOpen ? " on" : "")}
                  onClick={openPolicy}
                >{polOpen ? "Hide routing policy" : "Routing policy"}</button>
                {/* Shown open or closed: whether a rule is in force, and
                    whether it still describes the directory, is the answer
                    someone came here for. */}
                <div className="set-hint">
                  {ignored === 0
                    ? "Nothing excluded."
                    : `Excluding ${ignored} provider${ignored === 1 ? "" : "s"}`}
                  {pol.resolved_at > 0 && ` · directory read ${ago(pol.resolved_at)}`}
                  {stale && " · out of date — new hosts are not covered"}
                </div>
              </div>
              {polOpen && draft && (
                <div className="pol-body">
                  {dir === null && !polErr && (
                    <div className="set-hint">Asking OpenRouter which hosts it knows…</div>
                  )}
                  <div className="set-label">Refuse these countries</div>
                  <div className="pol-chips">
                    {countries.map((c) => (
                      <button
                        key={c}
                        className={"pol-chip" + (draft.countries.includes(c) ? " on" : "")}
                        title={dir === null ? c : `${countryCount(c)} hosts name ${c}`}
                        onClick={() => toggleCountry(c)}
                      >{c}</button>
                    ))}
                  </div>
                  <label className="pol-check">
                    <input
                      type="checkbox"
                      checked={draft.unknown}
                      onChange={(e) => editDraft((d) => ({ ...d, unknown: e.target.checked }))}
                    />
                    Also block providers that report no country
                    {dir !== null && (
                      <span className="pol-count">({unknownCount} providers)</span>
                    )}
                  </label>
                  <div className="set-hint">
                    A country matches a host's headquarters or any of its datacenters,
                    so a company registered elsewhere is still caught by where it runs.
                  </div>
                  <div className="set-label">Blocked by name</div>
                  {draft.providers.length > 0 && (
                    <div className="pol-chips">
                      {draft.providers.map((s) => (
                        <button
                          key={s}
                          className="pol-chip on"
                          title={`Stop blocking ${s}`}
                          onClick={() => editDraft((d) => ({
                            ...d,
                            providers: d.providers.filter((x) => x !== s),
                          }))}
                        >{s} ✕</button>
                      ))}
                    </div>
                  )}
                  {/* The chips remove; this adds. Without it the list could
                      only ever shrink, which is what a hand-edited
                      providers.json would leave behind. */}
                  <div className="pol-block">
                    <input
                      className="set-input"
                      placeholder="provider slug — e.g. baidu"
                      value={blockSlug}
                      onChange={(e) => setBlockSlug(e.target.value)}
                      onKeyDown={(e) => { if (e.key === "Enter") addBlocked(); }}
                    />
                    <button
                      className="act-btn"
                      disabled={!blockSlug.trim()}
                      onClick={addBlocked}
                    >Block</button>
                  </div>
                  <div className="set-hint">
                    A name blocks that host for every model, whatever country it
                    reports. The slug is the one a pin uses — <code>deepinfra</code>,
                    not "DeepInfra".
                  </div>
                  <div className="pol-caps">
                    {(["prompt", "completion"] as const).map((side) => (
                      <label key={side} className="pol-cap">
                        {side === "prompt" ? "Default max $/M in" : "Default max $/M out"}
                        <input
                          className="set-input" type="number" step="0.01" min="0"
                          placeholder="no cap"
                          value={draft[side].text}
                          onChange={(ev) => {
                            const text = ev.target.value;
                            const bad = ev.target.validity.badInput;
                            editDraft((d) => ({ ...d, [side]: { text, bad } }));
                          }}
                        />
                      </label>
                    ))}
                  </div>
                  <div className="set-hint">
                    The default applies to every model that does not set its own ceiling.
                    A model that sets one replaces this rather than merging with it.
                  </div>
                  {polErr && <div className="set-notice">{polErr}</div>}
                  <div className="pol-acts">
                    <button className="set-done" disabled={polBusy} onClick={savePolicy}>
                      {polBusy ? "Saving…" : "Save policy"}
                    </button>
                  </div>
                  <div className="set-hint">
                    Saving reads the directory and compiles the list of hosts to refuse,
                    which is also how an out-of-date list is brought current. A directory
                    that will not load fails the save rather than storing a rule that
                    blocks nothing.
                  </div>
                </div>
              )}
            </div>
          )}
          {/* The other half of the policy above: that one says what will
              happen, this one says what already did. */}
          {isOpenRouter && browsingProv && (
            <div className="acty">
              <div className="acty-head">
                <button
                  className={"act-btn" + (actOpen ? " on" : "")}
                  onClick={() => setActOpen(!actOpen)}
                >{actOpen ? "Hide activity" : "What actually ran"}</button>
                <div className="set-hint">
                  {browsingProv.has_management_key
                    ? `Management key ${browsingProv.management_key_hint
                      ? `…${browsingProv.management_key_hint}` : "saved"}`
                    : "Needs a management key"}
                </div>
              </div>
              {actOpen && (
                <div className="acty-panel">
                  <RoutingActivity prov={browsingProv} />
                  <div className="acty-key">
                    <input
                      className="set-input" type="password" autoComplete="off"
                      placeholder={browsingProv.has_management_key
                        ? "Replace the management key" : "OpenRouter management key"}
                      value={mgmtKey}
                      onChange={(e) => setMgmtKey(e.target.value)}
                      onKeyDown={(e) => {
                        if (e.key === "Enter" && mgmtKey.trim()) saveMgmtKey(mgmtKey.trim());
                      }}
                    />
                    <button
                      className="act-btn"
                      disabled={mgmtBusy || !mgmtKey.trim()}
                      onClick={() => saveMgmtKey(mgmtKey.trim())}
                    >{mgmtBusy ? "Saving…" : "Save key"}</button>
                    {browsingProv.has_management_key && (
                      <button
                        className="act-btn danger"
                        disabled={mgmtBusy}
                        title="Clear the stored management key"
                        onClick={() => saveMgmtKey("")}
                      >Forget</button>
                    )}
                  </div>
                  {mgmtErr && <div className="set-notice">{mgmtErr}</div>}
                  <div className="set-hint">
                    Activity needs an OpenRouter management key — create one at
                    {" "}<code>openrouter.ai/settings/keys</code>. An inference key is
                    refused for this one call and works everywhere else. It is stored
                    beside the API key and never shown again.
                  </div>
                </div>
              )}
            </div>
          )}
          <div className="mb-controls">
            <input
              className="set-input mb-search"
              placeholder="Search models"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
            />
            <div className="seg">
              {(["all", "free", "paid", "starred"] as const).map((f) => (
                <button
                  key={f}
                  className={"seg-btn" + (filter === f ? " on" : "")}
                  onClick={() => setFilter(f)}
                >{f === "all" ? "All" : f === "free" ? "Free"
                  : f === "paid" ? "Paid" : "Starred"}</button>
              ))}
            </div>
            <select
              className="set-select mb-ctx"
              value={minCtx}
              onChange={(e) => setMinCtx(+e.target.value)}
            >
              <option value={0}>Any context</option>
              <option value={32000}>≥ 32K</option>
              <option value={128000}>≥ 128K</option>
              <option value={200000}>≥ 200K</option>
              <option value={1000000}>≥ 1M</option>
            </select>
          </div>

          {browseErr ? (
            <div className="set-notice">{browseErr}</div>
          ) : all === undefined ? (
            <div className="set-hint mb-wait">Asking the provider…</div>
          ) : (
            <>
              <div className="mb-body">
                <div className="mb-list">
                  {shown.map((m) => (
                    <button
                      key={m.id}
                      className={"mb-item" + (sel?.id === m.id ? " on" : "")}
                      onClick={() => setPicked(m.id)}
                    >
                      <span className="mb-item-name">{m.name ?? m.id}</span>
                      {browsingProv?.startup_models.includes(m.id) && (
                        <span className="mb-star" title="On the startup list">★</span>
                      )}
                      {isFree(m) && <span className="mb-free">free</span>}
                    </button>
                  ))}
                  {shown.length === 0 && (
                    <div className="set-hint mb-wait">Nothing matches those filters.</div>
                  )}
                </div>
                {sel && (
                  <div className="mb-card">
                    <div className="mb-card-name">{sel.name ?? sel.id}</div>
                    <div className="mb-card-id">
                      <code>{sel.id}</code>
                      <button className="act-btn" onClick={() => copyId(sel.id)}>
                        {copied ? "Copied" : "Copy id"}
                      </button>
                    </div>
                    <button
                      className={
                        "act-btn mb-startup" +
                        (browsingProv?.startup_models.includes(sel.id) ? " on" : "")
                      }
                      onClick={() => toggleStartup(sel.id)}
                    >
                      {browsingProv?.startup_models.includes(sel.id)
                        ? "★ On the startup list — remove"
                        : "☆ Add to startup list"}
                    </button>
                    {isOpenRouter && (
                      <div className="ep">
                        {browsingProv?.routes[sel.id]?.order[0] && (
                          <div className="mb-pin">
                            Pinned to {browsingProv.routes[sel.id].order[0]} — no fallback
                            {/* A pin is "only this host" and the policy is
                                "not this host" — together they leave nowhere
                                to route, so the contradiction is said here
                                rather than discovered as a failed request. */}
                            {pol?.resolved_ignore[browsingProv.routes[sel.id].order[0]] && (
                              <span className="mb-pin-warn">
                                — your policy now blocks this host; every request will fail
                              </span>
                            )}
                            <button className="act-btn" onClick={() => pin(sel.id, null)}>
                              Unpin
                            </button>
                          </div>
                        )}
                        <button className="act-btn" onClick={() => openEndpoints(sel.id)}>
                          {epOpen === sel.id ? "Hide providers"
                            : `Providers${endpoints[sel.id] ? ` (${endpoints[sel.id].length})` : ""}`}
                        </button>
                        {epErr?.model === sel.id && (
                          <div className="set-notice">{epErr.msg}</div>
                        )}
                        {epOpen === sel.id && (
                          <div className="ep-list">
                            {endpoints[sel.id] === undefined ? (
                              epErr?.model !== sel.id
                                && <div className="set-hint mb-wait">Asking the provider…</div>
                            ) : (
                              <>
                                {/* Cheapest first, and the headings say what
                                    each column of figures is — "1M / 99.6%"
                                    told you nothing on its own. */}
                                <div className="ep-head">
                                  <span>Host</span>
                                  <span>Quant</span>
                                  <span>In / out $/M</span>
                                  <span>Ctx</span>
                                  <span>Up</span>
                                  <span />
                                </div>
                                {endpoints[sel.id].map((e) => (
                                  <button
                                    key={e.tag}
                                    className={"ep-row"
                                      + (e.excluded ? " off" : "")
                                      + (browsingProv?.routes[sel.id]?.order[0] === e.slug ? " on" : "")}
                                    disabled={!!e.excluded}
                                    title={e.excluded ? `Excluded: ${e.excluded}` : `Pin ${e.provider_name}`}
                                    onClick={() => pin(sel.id, e.slug)}
                                  >
                                    <span className="ep-name">{e.provider_name}</span>
                                    <span className="ep-tag">{e.quantization ?? ""}</span>
                                    <span className="ep-price">
                                      {fmtPrice(e.prompt_price)} / {fmtPrice(e.completion_price)}
                                    </span>
                                    <span className="ep-ctx">{fmtCtx(e.context_length)}</span>
                                    <span className="ep-up">
                                      {e.uptime_30m == null ? "—" : `${e.uptime_30m.toFixed(1)}%`}
                                    </span>
                                    {e.excluded && <span className="ep-off">{e.excluded}</span>}
                                  </button>
                                ))}
                                <div className="set-hint">
                                  A pin names a provider, not a quantization — <code>wafer</code> and
                                  {" "}<code>wafer/fast</code> are two rows and one slug.
                                </div>
                              </>
                            )}
                            <div className="ep-caps">
                              {(["prompt", "completion"] as const).map((side) => (
                                <label key={side} className="ep-cap">
                                  {side === "prompt" ? "Max $/M in" : "Max $/M out"}
                                  <input
                                    className="set-input" type="number" step="0.01" min="0"
                                    placeholder={capHint(sel.id, side)}
                                    value={capText(sel.id, side)}
                                    onChange={(ev) =>
                                      setCap({
                                        key: capKey(sel.id, side),
                                        text: ev.target.value,
                                        bad: ev.target.validity.badInput,
                                      })}
                                    onBlur={(ev) => commitCap(sel.id, side, ev.currentTarget)}
                                    onKeyDown={(ev) => {
                                      if (ev.key === "Enter") commitCap(sel.id, side, ev.currentTarget);
                                    }}
                                  />
                                </label>
                              ))}
                            </div>
                            <div className="set-hint">
                              A ceiling here replaces the account default for this model rather
                              than merging with it, and applies whether or not a host is pinned.
                            </div>
                          </div>
                        )}
                      </div>
                    )}
                    <div className="mb-meta">
                      <span className="mb-k">Context</span>
                      <span className="mb-v">{fmtCtx(sel.context_length)}</span>
                      <span className="mb-k">Input</span>
                      <span className="mb-v">{fmtPrice(sel.prompt_price)}</span>
                      <span className="mb-k">Output</span>
                      <span className="mb-v">{fmtPrice(sel.completion_price)}</span>
                      {sel.modalities.length > 0 && (
                        <>
                          <span className="mb-k">Accepts</span>
                          <span className="mb-v">{sel.modalities.join(", ")}</span>
                        </>
                      )}
                    </div>
                    {sel.description && <div className="mb-desc">{sel.description}</div>}
                  </div>
                )}
              </div>
              <div className="mb-count">
                {shown.length === all.length
                  ? `${all.length} models`
                  : `${shown.length} of ${all.length} models`}
              </div>
            </>
          )}
        </div>
      )}

      <div className="prov-form">
        <div className="set-label">{editing ? `Edit ${name}` : "Add a provider"}</div>
        {!editing && (
          <div className="prov-presets">
            {PRESETS.map((s) => (
              <button
                key={s.name}
                className="prov-preset"
                onClick={() => { setName(s.name); setBaseUrl(s.base_url); }}
              >{s.name}</button>
            ))}
          </div>
        )}
        <input
          className="set-input" placeholder="Name"
          value={name} onChange={(e) => setName(e.target.value)}
        />
        <input
          className="set-input" placeholder="Base URL — https://openrouter.ai/api/v1"
          value={baseUrl} onChange={(e) => setBaseUrl(e.target.value)}
        />
        <input
          className="set-input" type="password" autoComplete="off"
          placeholder={editing ? "API key — leave blank to keep the saved one" : "API key"}
          value={apiKey} onChange={(e) => setApiKey(e.target.value)}
        />
        {error && <div className="set-notice">{error}</div>}
        <div className="prov-form-acts">
          <button className="set-done" disabled={busy} onClick={submit}>
            {busy ? "Saving…" : editing ? "Save changes" : "Add provider"}
          </button>
          {editing && <button className="act-btn" onClick={reset}>Cancel</button>}
        </div>
      </div>

      <div className="set-hint">
        Models on a startup list appear in the new-session menu and open as a
        chat console in a tab. Test proves the key and URL work before anything
        depends on them.
      </div>
    </div>
  );
}
