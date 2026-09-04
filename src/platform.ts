import { invoke as nativeInvoke, type InvokeArgs, type InvokeOptions } from "@tauri-apps/api/core";

export const windowsWsl = import.meta.env.VITE_WINDOWS_WSL === "1";

/** Shared panels keep their API; the Windows host routes work into Linux. */
export function invoke<T>(command: string, args?: InvokeArgs, options?: InvokeOptions): Promise<T> {
  return windowsWsl
    ? nativeInvoke<T>("workspace_rpc", { command, args: args ?? {} }, options)
    : nativeInvoke<T>(command, args, options);
}
