/** Terminal-style escaping preserves the CLI's pasted image/path recognition. */
export function attachmentPaths(paths: readonly string[]): string[] {
  return [...new Set(paths)].map(path => {
    if (!path || /[\x00-\x1f\x7f]/.test(path)) {
      throw new Error("This filename contains control characters and cannot be attached.");
    }
    return path.replace(/[^A-Za-z0-9_\-./~+:@%=]/g, char => "\\" + char);
  });
}
