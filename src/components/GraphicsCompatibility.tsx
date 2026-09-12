import { useEffect, useState } from "react";
import { graphicsSettings, graphicsSettingsSet, type GraphicsMode, type GraphicsSettings } from "../ipc";
import Row from "./SettingsRow";

export default function GraphicsCompatibility() {
  const [settings, setSettings] = useState<GraphicsSettings | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  useEffect(() => {
    let mounted = true;
    graphicsSettings().then(value => { if (mounted) setSettings(value); })
      .catch(reason => { if (mounted) setError(String(reason)); });
    return () => { mounted = false; };
  }, []);
  if (settings && !settings.supported) return null;
  const update = async (mode: GraphicsMode) => {
    setSaving(true);
    setError(null);
    try { setSettings(await graphicsSettingsSet(mode)); }
    catch (reason) { setError(String(reason)); }
    finally { setSaving(false); }
  };
  return <div className="sgroup">
    <div className="sgroup-title">Graphics compatibility</div>
    <div className="sgroup-rows">
      <Row label="NVIDIA Wayland compatibility"
        desc="Automatic applies the workaround when NVIDIA and Wayland are detected. On and Off save your choice. Changes take effect after you restart AiTerm.">
        <select className="set-select" aria-label="NVIDIA Wayland compatibility"
          value={settings?.mode ?? "automatic"} disabled={!settings || saving}
          onChange={event => void update(event.target.value as GraphicsMode)}>
          <option value="automatic">Automatic</option>
          <option value="on">On</option>
          <option value="off">Off</option>
        </select>
      </Row>
    </div>
    <div className="sgroup-foot" role="status">
      {!settings ? (error ? "Graphics settings unavailable." : "Reading graphics settings…") : settings.environment_override
        ? "Controlled by your launch environment. Remove that override to use this setting."
        : settings.active ? "Active for this run."
          : !settings.eligible ? "Not needed for the detected graphics environment."
          : "Not active for this run."}
      {settings?.restart_required && " Saved. Restart AiTerm when your sessions are ready."}
    </div>
    {error && <div className="sgroup-foot" role="alert">Graphics settings: {error}</div>}
  </div>;
}
