/**
 * "Bring in…": choose who joins the session and what they should look at.
 * The engine/model pickers are the launcher's own — API models and the
 * local llama.cpp provider included, as on the phone: the relay reads the
 * second agent's replies from its conversation and needs no CLI.
 *
 * The same steps as the phone's sheet, in the same order: who, how long,
 * whether the first agent proceeds as approved, then the note — the one
 * thing typed, so it gets the room.
 */
import { useState } from "react";
import StartControls, { StartChoice, useStartChoice } from "./StartControls";
import Icon from "./Icon";
import { Users, X } from "lucide-react";

export default function BringIn({ host, onGo, onClose, onOpenModelAccess }: {
  /** The first agent's name, for the copy ("Claude Code replies"). */
  host?: string;
  onGo: (choice: StartChoice, focus: string, rounds: number, auto: boolean) => void;
  onClose: () => void;
  onOpenModelAccess?: (providerId?: string) => void;
}) {
  const ctl = useStartChoice();
  const [focus, setFocus] = useState("");
  const [rounds, setRounds] = useState(2);
  const [auto, setAuto] = useState(false);
  const a = host ?? "the first agent";
  return (
    <div className="bringin">
      <div className="bringin-head">
        <Icon of={Users} size="sm" /> <span>Bring in a second agent</span>
        <button className="icon-btn" title="Close" onClick={onClose}><Icon of={X} size="sm" /></button>
      </div>
      <div className="bringin-body">
        <div className="bringin-lede">They read this session and talk it out with {a} right here. No files change; you decide after.</div>
        <StartControls ctl={ctl} onOpenModelAccess={onOpenModelAccess} />
        <div className="bringin-row">
          <label className="bringin-rounds">
            Length
            <select className="ns-select" value={rounds} onChange={(e) => setRounds(Number(e.target.value))}>
              <option value={1}>Quick — they read the session and write once</option>
              <option value={2}>Normal — they write, {a} replies, they answer</option>
              <option value={3}>Long — two replies back and forth</option>
            </select>
          </label>
        </div>
        <label className="bringin-auto" title="The last message tells the first agent you have already approved the outcome, so it proceeds instead of finishing and waiting on you">
          <input type="checkbox" checked={auto} onChange={(e) => setAuto(e.target.checked)} />
          Auto-approve
          <span className="bringin-auto-note">
            {auto ? `When they finish, ${a} proceeds as approved instead of waiting for you.` : `When they finish, ${a} waits for you before going on.`}
          </span>
        </label>
        <label className="bringin-focus-label">
          What should they look at?
          <textarea
            className="bringin-focus"
            rows={5}
            value={focus}
            onChange={(e) => setFocus(e.target.value)}
            placeholder="Optional — e.g. is this secure, is there a simpler way, challenge the plan. Blank asks for a general second view."
            spellCheck={false}
          />
        </label>
        <button className="tui-pick bringin-go" disabled={!ctl.ready} onClick={() => onGo(ctl.choice(), focus, rounds, auto)}>
          Bring them in
        </button>
        <div className="bringin-note">
          They open in a tab of their own, read this session's transcript, and write to {a} directly. What they are told is in Settings → Bring in.
        </div>
      </div>
    </div>
  );
}
