import Row from "./SettingsRow";

export default function WindowAppearance({ shown, onChange }: {
  shown: boolean;
  onChange: (shown: boolean) => void;
}) {
  return <div className="sgroup">
    <div className="sgroup-title">Window</div>
    <div className="sgroup-rows">
      <Row label="Show title bar" desc="Show the window title bar and its controls. Applies immediately. When hidden, drag the empty space in the toolbar to move the window.">
        <label className="sw">
          <input type="checkbox" aria-label="Show title bar" checked={shown}
            onChange={event => onChange(event.target.checked)} />
          <span className="sw-track"><span className="sw-knob" /></span>
        </label>
      </Row>
    </div>
  </div>;
}
