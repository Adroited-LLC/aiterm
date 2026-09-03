/**
 * Settings → Librarian: which model names the sessions, how it is reached,
 * whether it runs on its own, and what it has done so far.
 */
import { useEffect, useState } from "react";
import { AgentChoice, ProviderView, agentChoices, providerModels, providersList } from "../ipc";
import { LibrarianCtl } from "../librarian";
import { LibrarianSettings } from "../settings";
import Row from "./SettingsRow";
import Icon from "./Icon";
import AgentIcon from "./AgentIcon";
import ModelPicker from "./ModelPicker";
import { Loader2 } from "lucide-react";


function Switch({ checked, onChange, label }: { checked: boolean; onChange: (on: boolean) => void; label: string }) {
  return (
    <label className="sw" aria-label={label}>
      <input type="checkbox" checked={checked} onChange={(e) => onChange(e.target.checked)} />
      <span className="sw-track"><span className="sw-knob" /></span>
    </label>
  );
}

export default function LibrarianPane({ cfg, onChange, lib, onOpenModelAccess }: {
  cfg: LibrarianSettings;
  onChange: (l: LibrarianSettings) => void;
  lib: LibrarianCtl;
  onOpenModelAccess: () => void;
}) {
  const [providers, setProviders] = useState<ProviderView[]>([]);
  const [agents, setAgents] = useState<AgentChoice[]>([]);
  /** The chosen API provider's whole catalogue, for the model select. */
  const [catalogue, setCatalogue] = useState<string[] | null>(null);
  useEffect(() => {
    providersList().then(setProviders).catch(() => {});
    agentChoices().then(setAgents).catch(() => {});
  }, []);
  useEffect(() => {
    setCatalogue(null);
    if (cfg.engine !== "api" || !cfg.providerId) return;
    let live = true;
    providerModels(cfg.providerId).then((l) => { if (live) setCatalogue(l); }).catch(() => { if (live) setCatalogue([]); });
    return () => { live = false; };
  }, [cfg.engine, cfg.providerId]);
  const keyed = providers.filter((p) => p.has_key);
  const provider = keyed.find((p) => p.id === cfg.providerId) ?? null;
  const agent = agents.find((a) => a.id === cfg.engine) ?? null;
  const set = (patch: Partial<LibrarianSettings>) => onChange({ ...cfg, ...patch });
  const pickEngine = (engine: LibrarianSettings["engine"]) => {
    // A model id is in one engine's spelling; switching engines clears it,
    // and a CLI starts on the smallest thing it lists.
    const a = agents.find((x) => x.id === engine);
    const small = a?.models.find((m) => /haiku|mini|flash|fast|lite/i.test(m.id))?.id ?? "";
    set({ engine, model: engine === "api" ? "anthropic/claude-haiku-4.5" : small });
  };
  const looked = Object.keys(lib.store.sessions).length;
  const left = looked - lib.named;
  const n = (k: number, one: string, many = one + "s") => `${k} ${k === 1 ? one : many}`;

  return (
    <>
      <div className="sgroup">
        <div className="sgroup-rows">
          <Row
            label="Librarian"
            desc="Names sessions in the list. Engines that title their own sessions — Claude Code, Grok, Antigravity — are left alone. For the rest, a small model reads the conversation and writes a short title in place of the first prompt. A name you set by hand always wins."
          >
            <Switch checked={cfg.enabled} onChange={(on) => set({ enabled: on })} label="Librarian on" />
          </Row>
        </div>
      </div>
      {cfg.enabled && <>
      <div className="sgroup">
        <div className="sgroup-rows">
          <Row label="Runs through" desc="An installed CLI in its print mode uses the plan you already pay for. An API provider is for a model none of them serve.">
            <div className="lib-engines">
              {agents.map((a) => (
                <button
                  key={a.id}
                  className={"ns-agent-tab " + a.id + (cfg.engine === a.id ? " on" : "")}
                  onClick={() => pickEngine(a.id as LibrarianSettings["engine"])}
                >
                  <AgentIcon agent={a.id} size={13} /><span>{a.display_name}</span>
                </button>
              ))}
              <button
                className={"ns-agent-tab" + (cfg.engine === "api" ? " on" : "")}
                onClick={() => pickEngine("api")}
              >
                <AgentIcon agent="api" size={13} /><span>API</span>
              </button>
            </div>
          </Row>
          {cfg.engine === "api" && (
            <Row label="Provider" desc={keyed.length ? "A provider from Model access with a key saved." : "No provider has a key yet — add one in Model access."}>
              {keyed.length ? (
                <select className="ns-select" value={cfg.providerId} onChange={(e) => set({ providerId: e.target.value })}>
                  <option value="">Choose…</option>
                  {keyed.map((p) => <option key={p.id} value={p.id}>{p.name}</option>)}
                </select>
              ) : (
                <button className="tui-plain" onClick={onOpenModelAccess}>Open Model access</button>
              )}
            </Row>
          )}
          <Row label="Model" desc="Something small is plenty — it is naming, not coding. Haiku does it well.">
            {cfg.engine === "api" ? (
              <ModelPicker
                value={cfg.model}
                models={catalogue ?? []}
                loading={catalogue === null && !!cfg.providerId}
                placeholder={provider ? "Choose a model" : "Choose a provider first"}
                onPick={(m) => set({ model: m })}
              />
            ) : agent && agent.models.length > 0 ? (
              <select className="ns-select" value={cfg.model} onChange={(e) => set({ model: e.target.value })}>
                <option value="">{agent.display_name}'s default</option>
                {agent.models.map((m) => <option key={m.id} value={m.id}>{m.display_name}</option>)}
              </select>
            ) : (
              <input
                className="srow-input"
                value={cfg.model}
                onChange={(e) => set({ model: e.target.value })}
                placeholder="default"
                spellCheck={false}
              />
            )}
          </Row>
          <Row label="Run on its own" desc="A few minutes after a session goes quiet, name it if it has no title yet. Otherwise only when you press Name now.">
            <Switch checked={cfg.auto} onChange={(on) => set({ auto: on })} label="Automatic" />
          </Row>
        </div>
      </div>

      <div className="sgroup">
        <div className="sgroup-title">So far</div>
        <div className="sgroup-rows">
          <Row
            label={looked === 0 ? "Nothing named yet" : `${n(lib.named, "session")} named${left > 0 ? `, ${left} left to ${left === 1 ? "its" : "their"} engine` : ""}`}
            desc={
              (lib.store.spent > 0 ? `About $${lib.store.spent.toFixed(3)} spent through API providers, where they reported a cost. ` : "") +
              (lib.pending.length ? `${n(lib.pending.length, "session")} waiting to be looked at.` : "Everything current has been looked at.")
            }
          >
            <button
              className="tui-pick"
              disabled={!lib.ready || lib.running || lib.pending.length === 0}
              onClick={() => void lib.run()}
              title={!lib.ready ? "Turn it on and choose how it runs first" : undefined}
            >
              {lib.running
                ? <><Icon of={Loader2} size="sm" className="spin" /> {lib.progress ? `${lib.progress.done} of ${lib.progress.total}` : "Reading…"}</>
                : `Name now${lib.pending.length ? ` (${lib.pending.length})` : ""}`}
            </button>
            {lib.running && <button className="tui-plain" onClick={lib.stop} title="Stop after the current one">Stop</button>}
          </Row>
          {lib.report && (
            <div className="sgroup-foot">
              Last run: {n(lib.report.done, "session")} named
              {lib.report.skipped ? `, ${lib.report.skipped} already titled by ${lib.report.skipped === 1 ? "its" : "their"} engine` : ""}
              {lib.report.remaining ? `, ${lib.report.remaining} still to go` : ""}
              {lib.report.cost > 0 ? `, $${lib.report.cost.toFixed(4)}` : ""}.
              {lib.report.errors.map((e, i) => <div key={i} className="lib-error">{e}</div>)}
            </div>
          )}
        </div>
      </div>

      <div className="sgroup">
        <div className="sgroup-rows">
          <Row label="Start over" desc="Forget every name it wrote — for a different model, or a first pass that went wrong. Sessions themselves, and names you set by hand, are untouched.">
            <button className="tui-plain" disabled={lib.running || looked === 0} onClick={() => void lib.forget()}>Forget everything</button>
          </Row>
        </div>
      </div>
      </>}
    </>
  );
}
