import { useEffect, useState } from "react";
import { graphicsSettings, graphicsSettingsSet, type GraphicsSettings } from "../ipc";
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
  const update = async (enabled: boolean) => {
    setSaving(true);
    setError(null);
    try { setSettings(await graphicsSettingsSet(enabled)); }
    catch (reason) { setError(String(reason)); }
    finally { setSaving(false); }
  };
  return <div className="sgroup">
    <div className="sgroup-title">Graphics compatibility</div>
    <div className="sgroup-rows">
      <Row label="NVIDIA Wayland compatibility"
        desc="Work around startup crashes on NVIDIA with Wayland. Applies only on matching systems. Changes take effect after you restart AiTerm.">
        <label className="sw">
          <input type="checkbox" aria-label="NVIDIA Wayland compatibility"
            checked={settings?.enabled ?? false} disabled={!settings || saving}
            onChange={event => void update(event.target.checked)} />
          <span className="sw-track"><span className="sw-knob" /></span>
        </label>
      </Row>
    </div>
    <div className="sgroup-foot" role="status">
      {!settings ? (error ? "Graphics settings unavailable." : "Reading graphics settings…") : settings.environment_override
        ? "Controlled by your launch environment. Remove that override to use this setting."
        : settings.active ? "Active for this run." : "Not active for this run."}
      {settings?.restart_required && " Saved. Restart AiTerm when your sessions are ready."}
    </div>
    {error && <div className="sgroup-foot" role="alert">Graphics settings: {error}</div>}
  </div>;
}
