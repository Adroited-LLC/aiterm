/**
 * Settings → Bring in: what the two agents are told when one is brought
 * into the other's session. Five short prompts, each editable; blank means
 * the one shipped.
 */
import { BRING_IN_DEFAULTS, BringInPrompts } from "../settings";
import Row from "./SettingsRow";

const FIELDS: { key: keyof BringInPrompts; label: string; rows: number }[] = [
  { key: "opening", label: "To the second agent, when it opens", rows: 5 },
  { key: "toFirst", label: "To the first agent, when a reply is expected", rows: 5 },
  { key: "toFirstLast", label: "To the first agent, the last message", rows: 5 },
  { key: "toSecond", label: "To the second agent, the first agent's reply", rows: 4 },
  { key: "approved", label: "Added to the last message when Auto-approve is on", rows: 2 },
];

export default function BringInPane({ prompts, onChange }: {
  prompts: BringInPrompts;
  onChange: (next: BringInPrompts) => void;
}) {
  const set = (key: keyof BringInPrompts, value: string) =>
    onChange({ ...prompts, [key]: value === BRING_IN_DEFAULTS[key] ? "" : value });
  return (
    <>
      <div className="sgroup">
        <div className="sgroup-rows">
          <Row
            label="Bring in a second agent"
            desc="Rounds are how many times the second agent speaks. With one, it reads the session and writes once; the first agent takes that in and carries on. With more, the first agent replies and they go back and forth, and the second agent's last message is handed back the same way. Nothing here limits what either may do; the second agent launches in whatever mode its engine is set to."
          >
            <span />
          </Row>
        </div>
      </div>
      <div className="sgroup">
        <div className="sgroup-title">What they are told</div>
        <div className="sgroup-rows">
          <div className="sgroup-foot">
            Placeholders: <code>{"{a}"}</code> the first agent, <code>{"{b}"}</code> the second, <code>{"{path}"}</code> the other agent's transcript on disk, <code>{"{focus}"}</code> what you asked for, <code>{"{text}"}</code> the message being passed on.
          </div>
          {FIELDS.map((f) => {
            const edited = prompts[f.key].trim() !== "" && prompts[f.key] !== BRING_IN_DEFAULTS[f.key];
            return (
              <div className="prompt-edit" key={f.key}>
                <div className="prompt-edit-head">
                  <span>{f.label}</span>
                  {edited && <span className="prompt-edit-mod">edited</span>}
                  <button className="linkish" onClick={() => set(f.key, "")} disabled={!prompts[f.key]}>Reset</button>
                </div>
                <textarea
                  className="prompt-edit-text"
                  value={prompts[f.key] || BRING_IN_DEFAULTS[f.key]}
                  onChange={(e) => set(f.key, e.target.value)}
                  spellCheck={false}
                  rows={f.rows}
                />
              </div>
            );
          })}
        </div>
      </div>
    </>
  );
}
