type Color = "Default" | { Indexed: number } | { Rgb: { r: number; g: number; b: number } };
export type RemoteCell = { text: string; width: number; continuation: boolean; foreground: Color; background: Color; attributes: Record<string, boolean> };
export type RemoteRow = { cells: RemoteCell[]; wrapped: boolean };
export type RemoteScreen = { tab_id: string; revision: number; cols: number; rows: number; visible: RemoteRow[]; scrollback: RemoteRow[]; cursor: { row: number; col: number; visible: boolean; shape: string }; modes: { application_cursor: boolean; bracketed_paste: boolean; alternate_screen: boolean } };
function colorCode(color: Color, foreground: boolean): string {
  const base = foreground ? 38 : 48;
  if (color === "Default") return foreground ? "39" : "49";
  if ("Indexed" in color) return `${base};5;${color.Indexed}`;
  return `${base};2;${color.Rgb.r};${color.Rgb.g};${color.Rgb.b}`;
}
// Screen cells are data, never terminal commands. Strip control characters before
// projecting the shared Rust screen model into the existing xterm renderer.
export function remoteRowsAnsi(rows: RemoteRow[]): string {
  return rows.map(row => row.cells.filter(cell => !cell.continuation).map(cell => {
    const a = cell.attributes;
    const codes = ["0", colorCode(cell.foreground, true), colorCode(cell.background, false)];
    for (const [name, code] of [["bold", "1"], ["faint", "2"], ["italic", "3"], ["underline", "4"], ["inverse", "7"], ["hidden", "8"], ["strikethrough", "9"]]) if (a[name]) codes.push(code);
    return `\x1b[${codes.join(";")}m${cell.text.replace(/[\x00-\x1f\x7f-\x9f]/g, "") || " ".repeat(cell.width)}`;
  }).join("") + "\x1b[0m").join("\r\n");
}
export function remoteScreenAnsi(screen: RemoteScreen): string {
  const cursor = screen.cursor;
  return `\x1b[?25l\x1b[?7l\x1b[H${remoteRowsAnsi(screen.visible)}\x1b[0m\x1b[${cursor.row + 1};${cursor.col + 1}H\x1b[${cursor.shape === "Bar" ? 6 : cursor.shape === "Underline" ? 4 : 2} q\x1b[?25${cursor.visible ? "h" : "l"}\x1b[?1${screen.modes.application_cursor ? "h" : "l"}\x1b[?2004${screen.modes.bracketed_paste ? "h" : "l"}`;
}
