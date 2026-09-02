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
        <label className="bringin-auto" title="The last message tells the first agent you have already approved the outcome, so it proceeds instead of finishing and waiting on you">
          <input type="checkbox" checked={auto} onChange={(e) => setAuto(e.target.checked)} />
          Auto-approve — the first agent proceeds as approved when they're done
        </label>
        <div className="bringin-foot">
          <label className="bringin-rounds">
            Rounds
            <select className="ns-select" value={rounds} onChange={(e) => setRounds(Number(e.target.value))}>
              <option value={1}>1 — they read and write once</option>
              <option value={2}>2 — they write, the first agent replies, they answer</option>
              <option value={3}>3 — two replies back and forth</option>
            </select>
          </label>
          <button className="tui-pick" disabled={!ctl.ready || ctl.isApi} onClick={() => onGo(ctl.choice(), focus, rounds, auto)}>
            Bring them in
          </button>
        </div>
        <div className="bringin-note">
          They open in a tab of their own, read this session's transcript, and write to the first agent directly. Rounds are how many times they speak; the first agent carries on after the last one. What they are told is in Settings → Bring in.
        </div>
      </div>
    </div>
  );
}
