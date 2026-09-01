/**
 * "Bring in…": choose who joins the session and what they should look at.
 * The engine/model pickers are the launcher's own.
 */
import { useState } from "react";
import StartControls, { StartChoice, useStartChoice } from "./StartControls";
import Icon from "./Icon";
import { Users, X } from "lucide-react";

export default function BringIn({ onGo, onClose, onOpenModelAccess }: {
  onGo: (choice: StartChoice, focus: string, rounds: number, auto: boolean) => void;
  onClose: () => void;
  onOpenModelAccess?: (providerId?: string) => void;
}) {
  const ctl = useStartChoice();
  const [focus, setFocus] = useState("");
  const [rounds, setRounds] = useState(2);
  const [auto, setAuto] = useState(false);
  return (
    <div className="bringin">
      <div className="bringin-head">
        <Icon of={Users} size="sm" /> <span>Bring in a second agent</span>
        <button className="icon-btn" title="Close" onClick={onClose}><Icon of={X} size="sm" /></button>
      </div>
      <div className="bringin-body">
        <StartControls ctl={ctl} onOpenModelAccess={onOpenModelAccess} allowApi={false} />
        <textarea
          className="bringin-focus"
          rows={2}
          value={focus}
          onChange={(e) => setFocus(e.target.value)}
          placeholder="What should they look at? — e.g. is this secure, is there a simpler way. Blank asks for a general second view."
          spellCheck={false}
        />
        <label className="bringin-auto" title="When the exchange ends, the first agent acts on the outcome — the agreed direction, or its judgment between the views — instead of waiting for you">
          <input type="checkbox" checked={auto} onChange={(e) => setAuto(e.target.checked)} />
          Auto-continue — act on the outcome without waiting
        </label>
        <div className="bringin-foot">
          <label className="bringin-rounds">
            Rounds
            <select className="ns-select" value={rounds} onChange={(e) => setRounds(Number(e.target.value))}>
              <option value={1}>1 — they speak, the first agent answers</option>
              <option value={2}>2 — and one reply back</option>
              <option value={3}>3</option>
            </select>
          </label>
          <button className="tui-pick" disabled={!ctl.ready || ctl.isApi} onClick={() => onGo(ctl.choice(), focus, rounds, auto)}>
            Bring them in
          </button>
        </div>
        <div className="bringin-note">
          They open in a tab of their own with this conversation in front of them, write to the first agent directly, and the two go back and forth. Neither edits files while they talk; you have both tabs and decide.
        </div>
      </div>
    </div>
  );
}
