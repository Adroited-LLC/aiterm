import test from "node:test";
import assert from "node:assert/strict";
import { remoteRowsAnsi, remoteScreenAnsi, type RemoteCell, type RemoteScreen } from "./desktopRemoteScreen.ts";
const cell = (text: string, changes: Partial<RemoteCell> = {}): RemoteCell => ({text,width:1,continuation:false,foreground:"Default",background:"Default",attributes:{},...changes});
test("remote cell data cannot inject terminal commands or clipboard writes",()=>{
 const ansi=remoteRowsAnsi([{wrapped:false,cells:[cell("safe\x1b]52;c;payload\x07\n\x9b31m")]}]);
 assert.ok(!ansi.includes("\x1b]52")); assert.ok(!ansi.includes("\x07")); assert.ok(!ansi.includes("\x9b"));
 assert.ok(ansi.includes("safe]52;c;payload31m"));
});
test("wide cell continuation is skipped and styles reset before the next cell",()=>{
 const ansi=remoteRowsAnsi([{wrapped:false,cells:[cell("界",{width:2,attributes:{bold:true},foreground:{Indexed:2}}),cell("",{continuation:true}),cell("a")]}]);
 assert.equal(ansi,"\x1b[0;38;5;2;49;1m界\x1b[0;39;49ma\x1b[0m");
});
test("snapshot projection restores cursor and input modes without terminal queries",()=>{
 const screen: RemoteScreen={tab_id:"a",revision:1,cols:10,rows:1,visible:[{wrapped:false,cells:[cell("x")]}],scrollback:[],cursor:{row:0,col:1,visible:true,shape:"Bar"},modes:{application_cursor:true,bracketed_paste:true,alternate_screen:false}};
 const ansi=remoteScreenAnsi(screen); assert.ok(ansi.includes("\x1b[1;2H")); assert.ok(ansi.includes("\x1b[?1h")); assert.ok(ansi.includes("\x1b[?2004h")); assert.ok(!ansi.includes("\x1b[6n"));
});

// Exercise the actual xterm parser. String checks alone miss stale cells after
// a shorter line, empty row, resize, or full-width last row.
test("repainting canonical frames erases old text and preserves the final row", async () => {
 const { Terminal } = (await import("@xterm/xterm")).default;
 const term = new Terminal({ cols: 10, rows: 3, allowProposedApi: true, scrollback: 0 });
 const write = (text: string) => new Promise<void>(resolve => term.write(text, resolve));
 const screen = (lines: string[]): RemoteScreen => ({ tab_id:"a", revision:1, cols:term.cols, rows:term.rows, visible:lines.map(text => ({wrapped:false,cells:[...text].map(c => cell(c))})), scrollback:[], cursor:{row:term.rows-1,col:2,visible:true,shape:"Block"},modes:{application_cursor:false,bracketed_paste:false,alternate_screen:false} });
 try {
   await write(remoteScreenAnsi(screen(["abcdefghij", "old output", "0123456789"])));
   await write(remoteScreenAnsi(screen(["new", "", "prompt> "])));
   assert.deepEqual(Array.from({length:3},(_,i)=>term.buffer.active.getLine(i)?.translateToString(true)), ["new", "", "prompt> "]);
   assert.equal(term.buffer.active.cursorY, 2);
   assert.equal(term.buffer.active.baseY, 0);
   term.resize(6, 2);
   await write(remoteScreenAnsi(screen(["short", "end"])));
   assert.deepEqual(Array.from({length:2},(_,i)=>term.buffer.active.getLine(i)?.translateToString(true)), ["short", "end"]);
 } finally { term.dispose(); }
});
