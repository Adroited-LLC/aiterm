/** Tauri encodes an entire file path as one URL segment. A page needs real
 * directory separators so relative scripts, styles and images resolve beside
 * it. Keep all other escaping (including literal %2F in a filename) intact.
 * The leading double slash is intentional: the asset handler removes the
 * first slash, leaving the absolute filesystem path.
 */
export function htmlPreviewSrc(fileUrl: string): string {
  return fileUrl.replace(/%2f/gi, "/");
}
