export interface ModalSize {
  width: number;
  height: number;
}

export const SETTINGS_MODAL_MIN_WIDTH = 560;
export const SETTINGS_MODAL_MIN_HEIGHT = 400;
export const SETTINGS_MODAL_MAX_WIDTH_RATIO = 0.94;
export const SETTINGS_MODAL_MAX_HEIGHT_RATIO = 0.92;

/** Keep a pointer-resized settings window usable and inside the current viewport. */
export function settingsModalSize(
  start: ModalSize,
  deltaX: number,
  deltaY: number,
  viewport: ModalSize,
): ModalSize {
  const maxWidth = Math.max(SETTINGS_MODAL_MIN_WIDTH, viewport.width * SETTINGS_MODAL_MAX_WIDTH_RATIO);
  const maxHeight = Math.max(SETTINGS_MODAL_MIN_HEIGHT, viewport.height * SETTINGS_MODAL_MAX_HEIGHT_RATIO);
  return {
    width: Math.min(maxWidth, Math.max(SETTINGS_MODAL_MIN_WIDTH, start.width + deltaX)),
    height: Math.min(maxHeight, Math.max(SETTINGS_MODAL_MIN_HEIGHT, start.height + deltaY)),
  };
}
