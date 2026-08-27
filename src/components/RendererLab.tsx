import { useEffect, useRef, useState } from "react";
import { Terminal } from "@xterm/xterm";
import { WebglAddon } from "@xterm/addon-webgl";
import { AppSettings, boldWeightFor, termFontFamily, termTheme } from "../settings";
import { rendererProbe } from "../ipc";
import Row from "./SettingsRow";

/** Sample text built to expose what the renderers actually differ on: stem
 *  weight at small sizes, thin diagonals, and how box-drawing edges resolve.
 *  The last line is bold, so emphasis can still be judged against body text at
 *  whatever weight is chosen above. */
const SAMPLE = [
  "\x1b[1m const \x1b[0m\x1b[36mrenderer\x1b[0m = caps.gpu ? \x1b[33m\"webgl\"\x1b[0m : \x1b[33m\"dom\"\x1b[0m;",
  " Illegible: Il1| O0o rn/m cl/d   \x1b[2mdimmed text\x1b[0m   3.14",
  " ┌─────────────┬───────────┐    \x1b[32m✔ passed\x1b[0m   \x1b[31m✘ failed\x1b[0m",
  " │ the quick   │ 0123456789│    λ → ∀ ≈ ±",
  " └─────────────┴───────────┘    \x1b[1mthis line is bold\x1b[0m",
].join("\r\n");

/** Long enough to clear the noise floor, short enough that the button never
 *  feels like it hung. */
const BURST_LINES = 4000;
const BURST =
  Array.from(
    { length: BURST_LINES },
    (_, i) =>
      `\x1b[2m${String(i).padStart(6, "0")}\x1b[0m the quick brown fox jumps over the lazy dog ` +
      `\x1b[36m0123456789\x1b[0m ++--==##`,
  ).join("\r\n") + "\r\n";

type Kind = "gpu" | "dom";
type Reading = { cpuMs: number; gpuMs: number | null };

const CAPTION: Record<Kind, string> = {
  gpu: "Rendered by the GPU's own glyph atlas — grayscale antialiasing, always",
  dom: "Rendered by the desktop's font stack — subpixel antialiasing and hinting",
};

export default function RendererLab({ settings, onPick }: {
  settings: AppSettings;
  /** The renderer buttons live inside this component because they sit between
   *  the sample they change and the measurement of what they cost. */
  onPick: (value: Kind) => void;
}) {
  const elRef = useRef<HTMLDivElement | null>(null);
  const termRef = useRef<Terminal | null>(null);
  const webglRef = useRef<WebglAddon | null>(null);
  const [busy, setBusy] = useState(false);
  /** Kept per renderer so both halves survive switching between them — the
   *  comparison needs the other side, which is only measurable while selected. */
  const [readings, setReadings] = useState<Partial<Record<Kind, Reading>>>({});
  const [note, setNote] = useState<string | null>(null);
  /** A WebGL context that failed to attach would leave this pane quietly
   *  rendering as DOM while captioned as GPU. Tracked so it can be said out
   *  loud instead of shown as a silent lie. */
  const [gpuFailed, setGpuFailed] = useState(false);

  useEffect(() => {
    if (!elRef.current || termRef.current) return;
    const term = new Terminal({
      fontFamily: termFontFamily(settings),
      fontSize: settings.termFontSize,
      lineHeight: settings.termLineHeight,
      fontWeight: settings.termFontWeight,
      fontWeightBold: boldWeightFor(settings.termFontWeight),
      theme: termTheme(settings),
      rows: 5,
      cols: 64,
      cursorBlink: false,
      disableStdin: true,
      scrollback: 100,
      allowProposedApi: true,
    });
    termRef.current = term;
    term.open(elRef.current);
    term.write(SAMPLE);
    return () => {
      webglRef.current?.dispose();
      webglRef.current = null;
      term.dispose();
      termRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  /** Follows the setting exactly as the real terminals do, so this pane is the
   *  renderer in use and never a stand-in for it. */
  useEffect(() => {
    const term = termRef.current;
    if (!term) return;
    const live = webglRef.current;
    if (settings.termRenderer === "gpu" && !live) {
      try {
        const webgl = new WebglAddon();
        webgl.onContextLoss(() => setGpuFailed(true));
        term.loadAddon(webgl);
        webglRef.current = webgl;
        setGpuFailed(false);
      } catch {
        webglRef.current = null;
        setGpuFailed(true);
      }
    } else if (settings.termRenderer === "dom" && live) {
      live.dispose();
      webglRef.current = null;
    } else {
      return;
    }
    term.refresh(0, term.rows - 1);
  }, [settings.termRenderer]);

  // Font and theme changes are pushed onto the live instance, so this is always
  // the face actually chosen above.
  useEffect(() => {
    const term = termRef.current;
    if (!term) return;
    term.options.fontFamily = termFontFamily(settings);
    term.options.fontSize = settings.termFontSize;
    term.options.lineHeight = settings.termLineHeight;
    term.options.fontWeight = settings.termFontWeight;
    term.options.fontWeightBold = boldWeightFor(settings.termFontWeight);
    term.options.theme = termTheme(settings);
  }, [settings]);

  const measure = async () => {
    const term = termRef.current;
    if (!term || busy) return;
    setBusy(true);
    setNote(null);
    try {
      const before = await rendererProbe();
      if (!before.ok) {
        setNote("Couldn't read the renderer's counters on this system.");
        return;
      }
      await new Promise<void>((done) => term.write(BURST, () => done()));
      // write()'s callback means parsed, not painted. Two frames guarantees it
      // reached the glass before the counters are read again.
      await new Promise<void>((done) =>
        requestAnimationFrame(() => requestAnimationFrame(() => done())),
      );
      const after = await rendererProbe();
      setReadings((prev) => ({
        ...prev,
        [settings.termRenderer]: {
          cpuMs: Math.max(0, after.cpuMs - before.cpuMs),
          gpuMs:
            after.gpuMs !== null && before.gpuMs !== null
              ? Math.max(0, after.gpuMs - before.gpuMs)
              : null,
        },
      }));
      term.clear();
      term.write(SAMPLE);
    } finally {
      setBusy(false);
    }
  };

  const active = settings.termRenderer;
  const mine = readings[active];
  const gpu = readings.gpu;
  const dom = readings.dom;
  const ratio = gpu && dom && gpu.cpuMs > 0 ? dom.cpuMs / gpu.cpuMs : null;
  const other: Kind = active === "gpu" ? "dom" : "gpu";

  return (
    <div className="rlab">
      <div className="rlab-term" ref={elRef} />
      <div className="rlab-caption">
        {active === "gpu" && gpuFailed
          ? "WebGL didn't start — this is falling back to the desktop's font stack"
          : CAPTION[active]}
      </div>
      <Row
        label="Renderer"
        desc="GPU never uses the desktop's font smoothing; DOM does, and can look sharper"
      >
        <div className="seg">
          {([
            ["gpu", "GPU"],
            ["dom", "DOM"],
          ] as const).map(([value, label]) => (
            <button
              key={value}
              className={"seg-btn" + (active === value ? " on" : "")}
              onClick={() => onPick(value)}
            >{label}</button>
          ))}
        </div>
      </Row>
      {active === "dom" && (
        <div className="sgroup-foot">
          Switches every open terminal as you look at it. DOM is the renderer
          that used to leave cells behind after a redraw — if you see text that
          should be gone, that's the trade.
        </div>
      )}
      <div className="rlab-actions">
        <button className="rlab-btn" onClick={measure} disabled={busy}>
          {busy ? "Measuring…" : `Measure ${active.toUpperCase()}`}
        </button>
        <span className="rlab-hint">
          {mine
            ? `${mine.cpuMs} ms CPU for ${BURST_LINES.toLocaleString()} lines` +
              (mine.gpuMs !== null ? ` · ${mine.gpuMs} ms GPU` : "")
            : `${BURST_LINES.toLocaleString()} lines through this pane — your open terminals aren't touched`}
        </span>
      </div>
      {ratio !== null ? (
        <div className="sgroup-foot">
          DOM costs <b>{ratio.toFixed(1)}×</b> the CPU of GPU on this machine for the
          same output. What it buys is the subpixel antialiasing above, which GPU
          cannot do at all — a glyph atlas has no background to blend against.
        </div>
      ) : (
        mine && (
          <div className="sgroup-foot">
            Switch to {other.toUpperCase()} and measure again to compare the two.
          </div>
        )
      )}
      {note && <div className="sgroup-foot">{note}</div>}
    </div>
  );
}
