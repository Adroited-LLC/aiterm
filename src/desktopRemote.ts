import type { RemoteSession, RemoteTab } from "./desktopRemoteSessions";
import { invoke } from "./platform";
export type RemoteDesktop = { id: string; name: string; address: string; fingerprint: string };
import type { RemoteScreen, RemoteRow } from "./desktopRemoteScreen";
export { remoteScreenAnsi } from "./desktopRemoteScreen";
export type RemoteClientView = { revision: number; connection: string; desktop_id: string | null; error: string | null; tabs: RemoteTab[]; sessions: RemoteSession[]; stars: string[]; activity: Record<string, string>; agent_caps: Record<string, {resume?: boolean; fork?: boolean; delete?: boolean}>; preview_session: string | null; preview_messages: {role: string; text: string}[]; preview_loading: boolean; selected_tab: string | null; has_focus: boolean; attachment_id: string | null; screen: RemoteScreen | null; scrollback: RemoteRow[] };
export const desktopList = () => invoke<RemoteDesktop[]>("desktop_client_list");
export const desktopWatch = (after?: number) => invoke<RemoteClientView>("desktop_client_watch", { after: after ?? null });
export const desktopConnect = (id: string) => invoke<void>("desktop_client_connect", { id });
export const desktopDisconnect = () => invoke<void>("desktop_client_disconnect");
export const desktopPair = (path: string, name: string) => invoke<RemoteDesktop>("desktop_client_pair", { path, name });
export const desktopCancelPairing = () => invoke<void>("desktop_client_cancel_pairing");
export const desktopForget = (id: string) => invoke<void>("desktop_client_forget", { id });
export const desktopAttach = (tabId: string) => invoke<void>("desktop_client_attach", { tabId });
export const desktopFocus = () => invoke<void>("desktop_client_focus");
export const desktopInput = (data: string) => invoke<void>("desktop_client_input", { data });
export const desktopResize = (cols: number, rows: number) => invoke<void>("desktop_client_resize", { cols, rows });
export const desktopScrollback = (offset: number) => invoke<RemoteRow[]>("desktop_client_scrollback", { offset });

export const desktopType = (data: string, cols: number, rows: number, attachmentId: string) => invoke<void>("desktop_client_type", { data, cols, rows, attachmentId });
export const desktopRestore = () => invoke<void>("desktop_client_restore");

export const desktopSession = (action: string, sessionId: string, title?: string, on?: boolean) => invoke<void>("desktop_client_session", { action, sessionId, title: title ?? null, on: on ?? null });
