import { useEffect, useState } from "react";
import {
  ModelCard, ProviderView, providerDelete, providerModelCards, providerModels,
  providerSave, providerStartupSet, providersList,
} from "../ipc";

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

  const browse = async (p: ProviderView) => {
    if (browsing === p.id) {
      setBrowsing(null); // second click folds it away
      return;
    }
    setBrowsing(p.id);
    setBrowseErr(null);
    setPicked(null);
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
