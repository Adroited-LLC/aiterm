import { useEffect, useState } from "react";
import { invoke } from "../platform";
import Row from "./SettingsRow";

type Settings = {
  supported: boolean;
  show_app_title_bar: boolean;
  restart_required: boolean;
  environment_override: boolean;
};

export default function WindowAppearance() {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  useEffect(() => {
    let mounted = true;
    invoke<Settings>("window_appearance_settings")
      .then(value => { if (mounted) setSettings(value); })
      .catch(reason => { if (mounted) setError(String(reason)); });
    return () => { mounted = false; };
  }, []);
  if (settings && !settings.supported) return null;
  const update = async () => {
    if (!settings || saving) return;
    setSaving(true);
    setError(null);
    try {
      setSettings(await invoke<Settings>("window_appearance_settings_set", {
        showAppTitleBar: !settings.show_app_title_bar,
      }));
    } catch (reason) { setError(String(reason)); }
    finally { setSaving(false); }
  };
  return <div className="sgroup">
    <div className="sgroup-title">Window</div>
    <div className="sgroup-rows">
      <Row label="Show app title bar" desc="Turn off to use KDE’s title bar, frame, and glow instead. Saved for this computer; takes effect after restarting AiTerm.">
        <label className="sw">
          <input type="checkbox" aria-label="Show app title bar"
            checked={settings?.show_app_title_bar ?? true}
            disabled={!settings || saving || settings.environment_override}
            onChange={() => void update()} />
          <span className="sw-track"><span className="sw-knob" /></span>
        </label>
      </Row>
    </div>
    {!settings && !error && <div className="sgroup-foot" role="status">Reading window settings…</div>}
    {settings?.environment_override && <div className="sgroup-foot">Controlled by GTK_CSD in your launch environment.</div>}
    {settings?.restart_required && <div className="sgroup-foot" role="status">Saved. Restart AiTerm when your sessions are ready.</div>}
    {error && <div className="sgroup-foot" role="alert">Window settings: {error}</div>}
  </div>;
}
