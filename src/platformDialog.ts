import { open as nativeOpen, type OpenDialogOptions } from "@tauri-apps/plugin-dialog";
import { windowsWsl } from "./platform";

let chooseFolder: (() => Promise<string | null>) | undefined;
export function registerFolderPicker(picker: () => Promise<string | null>) { chooseFolder = picker; }
export function open(options: OpenDialogOptions) {
  if (windowsWsl && options.directory) {
    if (!chooseFolder) return Promise.reject(new Error("The Linux workspace is still starting."));
    return chooseFolder().then(path => options.multiple && path ? [path] : path);
  }
  return nativeOpen(options);
}
