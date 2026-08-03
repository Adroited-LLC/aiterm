import { useEffect, useRef, useState } from "react";
import { Terminal } from "@xterm/xterm";
import { WebglAddon } from "@xterm/addon-webgl";
import { AppSettings, boldWeightFor, termFontFamily, termTheme } from "../settings";
import { rendererProbe } from "../ipc";

/** Sample text chosen to expose the thing the renderers actually differ on:
 *  stem weight at small sizes, and how thin diagonals and box-drawing edges
 *  resolve. Prose alone hides it. */
const SAMPLE = [
  "\x1b[1m const \x1b[0m\x1b[36mrenderer\x1b[0m = caps.gpu ? \x1b[33m\"webgl\"\x1b[0m : \x1b[33m\"dom\"\x1b[0m;",
  " Illegible: Il1| O0o rn/m cl/d  \x1b[2mdimmed text\x1b[0m  \x1b[1mbold text\x1b[0m",
  " ┌─────────────┬───────────┐   \x1b[32m✔ passed\x1b[0m   \x1b[31m✘ failed\x1b[0m",
  " │ the quick   │ 0123456789│   λ → ∀ ≈ ± ∑",
  " └─────────────┴───────────┘",
].join("\r\n");

/** Long enough that the difference clears the noise floor, short enough that
 *  the button never feels like it hung. */
const BURST_LINES = 4000;
const BURST = Array.from(
  { length: BURST_LINES },
  (_, i) =>
    `\x1b[2m${String(i).padStart(6, "0")}\x1b[0m the quick brown fox jumps over the lazy dog ` +
    `\x1b[36m0123456789\x1b[0m ++--==##`,
).join("\r\n") + "\r\n";

type Reading = { cpuMs: number; gpuMs: number | null };

export default function RendererLab({ settings }: { settings: AppSettings }) {
  const elRef = useRef<HTMLDivElement | null>(null);
  const termRef = useRef<Terminal | null>(null);
  const webglRef = useRef<WebglAddon | null>(null);
  const [busy, setBusy] = useState(false);
  /** Kept per renderer so the two survive switching between them — the whole
   *  point is the comparison, which needs both halves at once. */
  const [readings, setReadings] = useState<Partial<Record<"gpu" | "dom", Reading>>>({});
  const [failed, setFailed] = useState(false);

  // Build once. Font and theme changes are pushed onto the live instance
  // below, the same way the real terminal handles them.
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
      scrollback: 200,
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

  // Follow the renderer setting, exactly as the real terminals do, so what is
  // on screen here is what is on screen there.
  useEffect(() => {
    const term = termRef.current;
    if (!term) return;
    const live = webglRef.current;
    if (settings.termRenderer === "gpu" && !live) {
      try {
        const webgl = new WebglAddon();
        webgl.onContextLoss(() => webgl.dispose());
        term.loadAddon(webgl);
        webglRef.current = webgl;
      } catch {
        webglRef.current = null;
      }
    } else if (settings.termRenderer === "dom" && live) {
      live.dispose();
      webglRef.current = null;
    } else {
      return;
    }
    term.refresh(0, term.rows - 1);
  }, [settings.termRenderer]);

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
    setFailed(false);
    try {
      const before = await rendererProbe();
      if (!before.ok) {
        setFailed(true);
        return;
      }
      await new Promise<void>((done) => term.write(BURST, () => done()));
      // write()'s callback means parsed, not painted. Two frames guarantees the
      // renderer has actually put it on the glass before the counters are read.
      await new Promise<void>((done) =>
        requestAnimationFrame(() => requestAnimationFrame(() => done())),
      );
      const after = await rendererProbe();
      if (!after.ok) {
        setFailed(true);
        return;
      }
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

  const gpu = readings.gpu;
  const dom = readings.dom;
  const other = settings.termRenderer === "gpu" ? "DOM" : "GPU";
  const ratio =
    gpu && dom && gpu.cpuMs > 0 ? dom.cpuMs / gpu.cpuMs : null;

  return (
    <div className="rlab">
      <div className="rlab-term" ref={elRef} />
      <div className="rlab-actions">
        <button className="rlab-btn" onClick={measure} disabled={busy}>
          {busy ? "Measuring…" : `Measure ${settings.termRenderer.toUpperCase()}`}
        </button>
        <span className="rlab-hint">
          {gpu && dom
            ? `${BURST_LINES.toLocaleString()} lines rendered`
            : `Measure both to compare — ${other} still needs a run`}
        </span>
      </div>
      {(gpu || dom) && (
        <div className="rlab-results">
          {(["gpu", "dom"] as const).map((key) => {
            const r = readings[key];
            return (
              <div key={key} className={"rlab-cell" + (settings.termRenderer === key ? " on" : "")}>
                <span className="rlab-key">{key.toUpperCase()}</span>
                <span className="rlab-val">{r ? `${r.cpuMs} ms CPU` : "—"}</span>
                {r?.gpuMs !== null && r?.gpuMs !== undefined && (
                  <span className="rlab-sub">{r.gpuMs} ms GPU</span>
                )}
              </div>
            );
          })}
        </div>
      )}
      {ratio !== null && (
        <div className="sgroup-foot">
          DOM costs <b>{ratio.toFixed(1)}×</b> the CPU of GPU on this machine for the
          same output. It buys subpixel antialiasing, which GPU cannot do at all —
          its glyph atlas has no background to blend against. Worth it when text
          quality matters more than a fast-scrolling log.
        </div>
      )}
      {failed && (
        <div className="sgroup-foot">Couldn't read the renderer's counters on this system.</div>
      )}
    </div>
  );
}
