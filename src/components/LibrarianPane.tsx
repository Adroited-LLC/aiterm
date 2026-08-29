/**
 * Settings → Librarian: which model reads the sessions, how it is reached,
 * whether it runs on its own, and what it has done so far.
 */
import { useEffect, useState } from "react";
import { AgentChoice, ProviderView, agentChoices, librarianDefaultPrompts, providerModels, providersList } from "../ipc";
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
  const [defaults, setDefaults] = useState<{ catalogue: string; tidy: string } | null>(null);
  const [showPrompts, setShowPrompts] = useState(false);
  useEffect(() => {
    providersList().then(setProviders).catch(() => {});
    agentChoices().then(setAgents).catch(() => {});
    librarianDefaultPrompts().then(setDefaults).catch(() => {});
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
  const counted = Object.keys(lib.store.sessions).length;
  const threads = Object.keys(lib.store.threads).length;

  return (
    <>
      <div className="sgroup">
        <div className="sgroup-rows">
          <Row
            label="Let a model catalogue sessions"
            desc="Reads each session once — the opening prompt, the last exchange, the files touched — and writes a short name, a few tags, the thread of work it belongs to, and where it left off. That feeds the Threads tab and, if you like, the names in the session list."
          >
            <Switch checked={cfg.enabled} onChange={(on) => set({ enabled: on })} label="Librarian on" />
          </Row>
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
          <Row label="Run on its own" desc="A minute or so after sessions go quiet, read any it has not seen. Otherwise only when you press Catalogue.">
            <Switch checked={cfg.auto} onChange={(on) => set({ auto: on })} label="Automatic" />
          </Row>
          <Row label="Tidy up after each run" desc="Sessions are read eight at a time, so the same project can end up in several threads. This takes one look at everything afterwards and merges them, and files loose sessions where they belong.">
            <Switch checked={cfg.tidyAfterRun} onChange={(on) => set({ tidyAfterRun: on })} label="Tidy after run" />
          </Row>
          <Row label="Use its names in the session list" desc="In place of the raw first prompt. The original stays in the tooltip.">
            <Switch checked={cfg.renameRows} onChange={(on) => set({ renameRows: on })} label="Rename rows" />
          </Row>
          <Row label="Show the Threads tab" desc="The fourth tab in the sidebar. Off hides the tab; the librarian still runs and its names still show in the list.">
            <Switch checked={cfg.showThreadsTab} onChange={(on) => set({ showThreadsTab: on })} label="Threads tab" />
          </Row>
        </div>
      </div>

      <div className="sgroup">
        <div className="sgroup-title">So far</div>
        <div className="sgroup-rows">
          <Row
            label={`${counted} session${counted === 1 ? "" : "s"} read, ${threads} thread${threads === 1 ? "" : "s"}`}
            desc={
              (lib.store.spent > 0 ? `About $${lib.store.spent.toFixed(3)} spent through API providers, where they reported a cost. ` : "") +
              (lib.pending.length ? `${lib.pending.length} session${lib.pending.length === 1 ? "" : "s"} waiting to be read.` : "Everything current has been read.")
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
                : `Catalogue now${lib.pending.length ? ` (${lib.pending.length})` : ""}`}
            </button>
            {lib.running && <button className="tui-plain" onClick={lib.stop} title="Stop after this batch">Stop</button>}
          </Row>
          {lib.report && (
            <div className="sgroup-foot">
              Last run: {lib.report.done} read{lib.report.remaining ? `, ${lib.report.remaining} still to go` : ""}
              {lib.report.cost > 0 ? `, $${lib.report.cost.toFixed(4)}` : ""}.
              {lib.report.errors.map((e, i) => <div key={i} className="lib-error">{e}</div>)}
            </div>
          )}
          <Row label="Tidy up now" desc={lib.tidyDue ? "Sessions have been read since the last tidy." : threads > 0 ? "Merge threads that are the same work; file loose sessions." : "Nothing to tidy yet."}>
            <button className="tui-plain" disabled={!lib.ready || lib.running || lib.tidying || threads === 0} onClick={() => void lib.tidy()}>
              {lib.tidying ? <><Icon of={Loader2} size="sm" className="spin" /> Tidying…</> : "Tidy up"}
            </button>
          </Row>
          {lib.tidyReport && !("error" in lib.tidyReport) && (
            <div className="sgroup-foot">
              Last tidy: {lib.tidyReport.threads_before} threads → {lib.tidyReport.threads_after}
              {lib.tidyReport.filed ? `, ${lib.tidyReport.filed} loose session${lib.tidyReport.filed === 1 ? "" : "s"} filed` : ""}.
            </div>
          )}
        </div>
      </div>

      <div className="sgroup">
        <div className="sgroup-title">Prompts</div>
        <div className="sgroup-rows">
          <Row label="What the model is told" desc="The system prompts for the two passes. Edit them to change how sessions are named and grouped; what follows each — the session excerpts, the thread list — is built by aiterm and stays the same. Blank means the one shipped.">
            <button className="tui-plain" onClick={() => setShowPrompts((v) => !v)}>{showPrompts ? "Hide" : "Edit"}</button>
          </Row>
          {showPrompts && defaults && (
            <>
              <div className="lib-prompt">
                <div className="lib-prompt-head">
                  <span>Reading sessions</span>
                  {cfg.promptCatalogue.trim() && cfg.promptCatalogue.trim() !== defaults.catalogue
                    ? <span className="lib-prompt-mod">edited</span> : null}
                  <button className="linkish" onClick={() => set({ promptCatalogue: "" })} disabled={!cfg.promptCatalogue}>Reset</button>
                </div>
                <textarea
                  className="lib-prompt-text"
                  value={cfg.promptCatalogue || defaults.catalogue}
                  onChange={(e) => set({ promptCatalogue: e.target.value === defaults.catalogue ? "" : e.target.value })}
                  spellCheck={false}
                  rows={12}
                />
              </div>
              <div className="lib-prompt">
                <div className="lib-prompt-head">
                  <span>Tidying up</span>
                  {cfg.promptTidy.trim() && cfg.promptTidy.trim() !== defaults.tidy
                    ? <span className="lib-prompt-mod">edited</span> : null}
                  <button className="linkish" onClick={() => set({ promptTidy: "" })} disabled={!cfg.promptTidy}>Reset</button>
                </div>
                <textarea
                  className="lib-prompt-text"
                  value={cfg.promptTidy || defaults.tidy}
                  onChange={(e) => set({ promptTidy: e.target.value === defaults.tidy ? "" : e.target.value })}
                  spellCheck={false}
                  rows={12}
                />
              </div>
              <div className="sgroup-foot">
                Each must end by asking for JSON in the shape aiterm reads — an array of one object per session for reading, an object with a <code>threads</code> list for tidying — or nothing will be written.
              </div>
            </>
          )}
        </div>
      </div>

      <div className="sgroup">
        <div className="sgroup-rows">
          <Row label="Start over" desc="Forget every name, tag and thread — for a different model, or a first pass that went wrong. Sessions themselves are untouched.">
            <button className="tui-plain" disabled={lib.running || counted === 0} onClick={() => void lib.forget()}>Forget everything</button>
          </Row>
        </div>
      </div>
    </>
  );
}
