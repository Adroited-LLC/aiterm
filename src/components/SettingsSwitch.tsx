/** The compact two-state control shared by settings panes. */
export default function SettingsSwitch({ checked, onChange, label, disabled = false }: {
  checked: boolean;
  onChange: (on: boolean) => void;
  label: string;
  disabled?: boolean;
}) {
  return (
    <label className={"sw" + (disabled ? " disabled" : "")} aria-label={label}>
      <input
        type="checkbox"
        checked={checked}
        disabled={disabled}
        onChange={(event) => onChange(event.target.checked)}
      />
      <span className="sw-track"><span className="sw-knob" /></span>
    </label>
  );
}
