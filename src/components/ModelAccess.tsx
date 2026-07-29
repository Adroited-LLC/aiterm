import { useEffect, useState } from "react";
import {
  ProviderView, providerDelete, providerModels, providerSave, providersList,
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
    } catch (e) {
      setTested((t) => ({ ...t, [p.id]: String(e) }));
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
      <div className="set-label">Model access</div>
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
                  {p.name}
                  <span className="prov-key">
                    {p.has_key ? (p.key_hint ? `key …${p.key_hint}` : "key saved") : "no key"}
                  </span>
                </div>
                <div className="prov-url">{p.base_url}</div>
                {tested[p.id] && <div className="prov-test">{tested[p.id]}</div>}
              </div>
              <div className="prov-acts">
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

      {/* Said plainly, because the alternative is someone adding a key and
          waiting for a model picker that is not built yet. */}
      <div className="set-hint">
        Configuration only for now — aiterm does not yet run sessions against
        these. Test proves the key and URL work so that when it does, this part
        is already known good.
      </div>
    </div>
  );
}
