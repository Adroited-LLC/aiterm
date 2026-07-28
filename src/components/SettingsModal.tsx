import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import ModelAccess from "./ModelAccess";
import {
  ACCENT_SWATCHES, AppSettings, DEFAULT_SETTINGS, PanelScales, THEMES, themeById,
} from "../settings";
import {
  FontFamily, FontPackage,
  AgentDetection, detectAgents, homeAbbrev,
  fontPackages, installFontFiles, installFontPackage, listFonts,
} from "../ipc";

interface Props {
  settings: AppSettings;
  onChange: (s: AppSettings) => void;
  onClose: () => void;
}

const PANEL_LABELS: { key: keyof PanelScales; label: string }[] = [
  { key: "sessions", label: "Sessions" },
  { key: "explorer", label: "Explorer" },
  { key: "git", label: "Repository" },
  { key: "agent", label: "Agent" },
];

type Tab = "appearance" | "fonts" | "agents" | "models";

/** Shows the characters that actually separate one coding font from another. */
const PREVIEW = "const ok = 0O1lI|; // {} => [a-z]* 3.14";

function ThemeCard({ id, active, onPick }: { id: string; active: boolean; onPick: () => void }) {
  const t = themeById(id);
  return (
    <button
      className={"theme-card" + (active ? " on" : "")}
      onClick={onPick}
      style={{ background: t.vars.bgPanel, borderColor: active ? t.vars.accent : t.vars.border }}
    >
      <div className="theme-strip">
        {[t.vars.accent, t.term.green, t.term.yellow, t.term.blue, t.term.magenta].map((c) => (
          <span key={c} style={{ background: c }} />
        ))}
      </div>
      <div className="theme-card-name" style={{ color: t.vars.text }}>{t.name}</div>
      <div className="theme-card-sub" style={{ color: t.vars.textFaint }}>Aa ❯ _</div>
    </button>
  );
}

export default function SettingsModal({ settings, onChange, onClose }: Props) {
  const [tab, setTab] = useState<Tab>("appearance");
  const [fonts, setFonts] = useState<FontFamily[]>([]);
  const [packages, setPackages] = useState<FontPackage[]>([]);
  /** Package currently installing, so its row can say so and not be clicked twice. */
  const [installing, setInstalling] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  // Detection spawns a process per installed agent, so it runs when this modal
  // opens and when explicitly re-checked — never on a timer. `null` is "not
  // asked yet", which is a different thing to show than an empty list.
  const [agents, setAgents] = useState<AgentDetection[] | null>(null);
  const refreshAgents = () => detectAgents().then(setAgents).catch(() => setAgents([]));
  useEffect(() => { if (tab === "agents" && agents === null) refreshAgents(); }, [tab, agents]);

  const set = (patch: Partial<AppSettings>) => onChange({ ...settings, ...patch });
  const setScale = (key: keyof PanelScales, v: number) =>
    set({ panelScale: { ...settings.panelScale, [key]: v } });

  const refreshFonts = () => {
    listFonts().then(setFonts).catch(() => {});
    fontPackages().then(setPackages).catch(() => {});
  };
  useEffect(refreshFonts, []);

  const monoFonts = fonts.filter((f) => f.mono);

  const installPackage = async (pkg: FontPackage) => {
    setInstalling(pkg.package);
    setNotice(null);
    try {
      await installFontPackage(pkg.package);
      refreshFonts();
      setNotice(`Installed ${pkg.name} — pick it under Terminal font.`);
    } catch (e) {
      setNotice(String(e));
    } finally {
      setInstalling(null);
    }
  };

  const installFiles = async () => {
    setNotice(null);
    try {
      const picked = await open({
        multiple: true,
        title: "Install font files",
        filters: [{ name: "Fonts", extensions: ["ttf", "otf", "ttc", "otc", "pfb"] }],
      });
      const paths = Array.isArray(picked) ? picked : picked ? [picked] : [];
      if (!paths.length) return;
      const n = await installFontFiles(paths);
      refreshFonts();
      setNotice(`Installed ${n} font file${n === 1 ? "" : "s"} to ~/.local/share/fonts.`);
    } catch (e) {
      setNotice(String(e));
    }
  };

  return (
    <div className="modal-overlay" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="modal settings-modal">
        <div className="modal-head">
          <span className="modal-title">Settings</span>
          <div className="set-tabs">
            <button
              className={"set-tab" + (tab === "appearance" ? " on" : "")}
              onClick={() => setTab("appearance")}
            >Appearance</button>
            <button
              className={"set-tab" + (tab === "fonts" ? " on" : "")}
              onClick={() => setTab("fonts")}
            >Fonts</button>
            <button
              className={"set-tab" + (tab === "agents" ? " on" : "")}
              onClick={() => setTab("agents")}
            >Agents</button>
            <button
              className={"set-tab" + (tab === "models" ? " on" : "")}
              onClick={() => setTab("models")}
            >Model access</button>
          </div>
          <button className="icon-btn" title="Close (Esc)" onClick={onClose}>✕</button>
        </div>
        <div className="modal-body">

          {tab === "appearance" && <>
            <div className="set-section">
              <div className="set-label">Theme</div>
              <div className="theme-grid">
                {THEMES.map((t) => (
                  <ThemeCard
                    key={t.id}
                    id={t.id}
                    active={settings.themeId === t.id}
                    onPick={() => set({ themeId: t.id })}
                  />
                ))}
              </div>
            </div>

            <div className="set-section">
              <div className="set-label">Accent color</div>
              <div className="accent-row">
                <button
                  className={"accent-swatch default" + (settings.accent === null ? " on" : "")}
                  title="Theme default"
                  onClick={() => set({ accent: null })}
                  style={{ background: themeById(settings.themeId).vars.accent }}
                >A</button>
                {ACCENT_SWATCHES.map((c) => (
                  <button
                    key={c}
                    className={"accent-swatch" + (settings.accent === c ? " on" : "")}
                    style={{ background: c }}
                    onClick={() => set({ accent: c })}
                  />
                ))}
              </div>
            </div>

            <div className="set-section">
              <div className="set-label">Panel sizes</div>
              {PANEL_LABELS.map(({ key, label }) => (
                <div key={key} className="set-slider-row">
                  <span className="set-slider-name">{label}</span>
                  <input
                    type="range" min={0.7} max={1.5} step={0.05}
                    value={settings.panelScale[key]}
                    onChange={(e) => setScale(key, +e.target.value)}
                  />
                  <span className="set-value">{Math.round(settings.panelScale[key] * 100)}%</span>
                </div>
              ))}
              <div className="set-hint">Ctrl + / Ctrl − zooms everything at once.</div>
            </div>
          </>}

          {tab === "fonts" && <>
            <div className="set-section set-cols">
              <div>
                <div className="set-label">Panel font</div>
                <select
                  className="set-select"
                  value={settings.uiFont}
                  onChange={(e) => set({ uiFont: e.target.value })}
                >
                  <option value="">System default</option>
                  {fonts.map((f) => (
                    <option key={f.name} value={f.name}>{f.name}</option>
                  ))}
                </select>
              </div>
              <div>
                <div className="set-label">
                  Terminal font
                  <span className="set-value">{monoFonts.length} monospace</span>
                </div>
                <select
                  className="set-select mono"
                  value={settings.termFont}
                  onChange={(e) => set({ termFont: e.target.value })}
                >
                  <option value="">System default</option>
                  {monoFonts.map((f) => (
                    <option key={f.name} value={f.name}>{f.name}</option>
                  ))}
                </select>
              </div>
            </div>

            <div className="set-section">
              <div className="set-label">
                Terminal font size
                <span className="set-value">{settings.termFontSize}px</span>
              </div>
              <input
                type="range" min={9} max={22} step={1}
                value={settings.termFontSize}
                onChange={(e) => set({ termFontSize: +e.target.value })}
              />
              {/* Same string the terminal would render, at the same size and
                  face — the only honest way to compare two coding fonts. */}
              <div
                className="font-preview"
                style={{
                  fontFamily: settings.termFont ? `"${settings.termFont}", monospace` : "monospace",
                  fontSize: settings.termFontSize,
                }}
              >{PREVIEW}</div>
            </div>

            <div className="set-section">
              <div className="set-label">Install a coding font</div>
              <div className="set-hint">
                From the Fedora repositories. Installing needs administrator rights —
                you may be asked for your password.
              </div>
              <div className="font-pkg-list">
                {packages.map((p) => (
                  <div key={p.package} className="font-pkg-row">
                    <div className="font-pkg-text">
                      <span
                        className="font-pkg-name"
                        style={p.installed ? { fontFamily: `"${p.name}", monospace` } : undefined}
                      >{p.name}</span>
                      <span className="font-pkg-note">{p.note}</span>
                    </div>
                    {p.installed ? (
                      <span className="font-pkg-done">installed</span>
                    ) : (
                      <button
                        className="font-pkg-install"
                        disabled={installing !== null}
                        onClick={() => installPackage(p)}
                      >{installing === p.package ? "installing…" : "Install"}</button>
                    )}
                  </div>
                ))}
              </div>
              <button className="set-file-install" onClick={installFiles}>
                Install from file…
              </button>
              <div className="set-hint">
                For fonts that are not packaged — Nerd Fonts, anything you downloaded.
                Copied into ~/.local/share/fonts; no administrator rights needed.
              </div>
              {notice && <div className="set-notice">{notice}</div>}
            </div>
          </>}

          {tab === "models" && <ModelAccess />}

          {tab === "agents" && <>
            <div className="set-section">
              <div className="set-label">
                Agents
                <button className="set-recheck" onClick={refreshAgents}>Re-check</button>
              </div>
              {/* Agents aiterm does not find are listed too. "Codex — not
                  installed" is the useful answer; an absence would leave you
                  wondering whether aiterm supports it at all. */}
              {agents === null ? (
                <div className="set-hint">Looking…</div>
              ) : (
                <div className="agent-list">
                  {agents.map((a) => (
                    <div key={a.id} className="agent-row">
                      <span className={"agent-dot" + (a.available ? " on" : "")} />
                      <div className="agent-text">
                        <div className="agent-name">
                          {a.display_name}
                          <span className="agent-state">
                            {a.available ? (a.version ?? "installed") : "not installed"}
                          </span>
                        </div>
                        {/* Which copy was found — worth showing when several
                            are installed and the wrong one is on PATH. */}
                        {a.path && <div className="agent-path">{homeAbbrev(a.path)}</div>}
                        {a.id === "codex" && a.available && (
                          <div className="agent-path">
                            Detected, but aiterm does not read Codex sessions yet.
                          </div>
                        )}
                      </div>
                    </div>
                  ))}
                  {agents.length === 0 && (
                    <div className="set-hint">Nothing reported.</div>
                  )}
                </div>
              )}
              <div className="set-hint">
                Read from PATH when this tab opens, not polled — install something
                and press Re-check.
              </div>
            </div>
          </>}

        </div>
        <div className="modal-foot">
          <button
            className="set-reset"
            onClick={() => onChange({ ...DEFAULT_SETTINGS })}
          >Reset to defaults</button>
          <button className="set-done" onClick={onClose}>Done</button>
        </div>
      </div>
    </div>
  );
}
