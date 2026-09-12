/** Formats decoded by the desktop webview. Match file paths, not URL suffixes. */
export function isImage(path: string): boolean {
  return /\.(png|jpe?g|svg|bmp|gif|webp|ico)$/i.test(path);
}

export function fitImageScale(width: number, height: number, availableWidth: number, availableHeight: number): number {
  if (width <= 0 || height <= 0 || availableWidth <= 0 || availableHeight <= 0) return 1;
  return Math.min(1, availableWidth / width, availableHeight / height);
}
