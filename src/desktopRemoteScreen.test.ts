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
