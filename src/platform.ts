import { invoke as nativeInvoke, convertFileSrc as nativeFileSrc, type InvokeArgs, type InvokeOptions } from "@tauri-apps/api/core";

export const windowsWsl = import.meta.env.VITE_WINDOWS_WSL === "1";
export type WslWorkspace = { distribution: string; home: string; shell: string };
let workspace: WslWorkspace | undefined;
export function setWorkspace(value: WslWorkspace) { workspace = value; }
export function getWorkspace() { return workspace; }
export function linuxPath(path: string) {
  if (!windowsWsl) return path;
  const unc = workspace && `\\\\wsl.localhost\\${workspace.distribution}\\`;
  if (unc && path.toLowerCase().startsWith(unc.toLowerCase())) return "/" + path.slice(unc.length).split("\\").join("/");
  if (/^[a-z]:[\\/]/i.test(path)) return `/mnt/${path[0].toLowerCase()}/${path.slice(3).split("\\").join("/")}`;
  return path;
}
export function convertFileSrc(path: string, protocol?: string) {
  if (windowsWsl && workspace && path.startsWith("/")) {
    // Keep directory separators in the URL so HTML relative assets resolve
    // beside their page. Windows accepts these forward slashes after the UNC root.
    return nativeFileSrc(`\\\\wsl.localhost\\${workspace.distribution}`, protocol) + path.split("/").map(encodeURIComponent).join("/");
  }
  return nativeFileSrc(path, protocol);
}
export function terminalOutputConsumed(channel: number, bytes: number) {
  if (windowsWsl) void nativeInvoke("workspace_control", { frame: { type: "ack", channel, bytes } }).catch(() => {});
}
export function terminalOutputClosed(channel: number) {
  if (windowsWsl) void nativeInvoke("workspace_control", { frame: { type: "channel_close", channel } }).catch(() => {});
}

/** Shared panels keep their API; the Windows host routes work into Linux. */
export function invoke<T>(command: string, args?: InvokeArgs, options?: InvokeOptions): Promise<T> {
  return windowsWsl
    ? nativeInvoke<T>("workspace_rpc", { command, args: args ?? {} }, options)
    : nativeInvoke<T>(command, args, options);
}
