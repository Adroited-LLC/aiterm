# Desktop image viewer

Click a PNG, JPG/JPEG, SVG, BMP, GIF, WebP, or ICO file in the file explorer,
or follow its file link in the terminal. It opens in a normal file tab.

- Fit contains the whole image without enlarging small images.
- 100% shows its natural size. Scroll horizontally or vertically to inspect it.
- The zoom buttons enlarge or reduce the image. With the image area focused,
  use + and - to zoom, 0 to fit, 1 for actual size, and arrow keys to pan.
- The footer shows the image dimensions. Transparent areas have a checkerboard.
- Reload reads the file again; the project watcher also refreshes visible images.
  Hidden image tabs catch up when selected and retain their chosen zoom.
- Open with the system app remains available, including for decoding failures.

Images use the existing scoped asset protocol on Linux and the active WSL
workspace on Windows. SVG is loaded as an image, not executable page markup.
This viewer is read-only; it never saves image bytes through the text editor.
Android's existing image preview is unchanged.

## Verification

Run `npm run test:ui`, `npm run build`, and `npm run build:windows-ui`.
For interactive checks, run the Vite dev server and open
`/tests/browser/image-viewer.html`. The small synthetic fixtures cover all
supported extensions plus a corrupt file; the SVG is large enough to test fit,
zoom, and scrolling. Check fit after resizing, keyboard panning at actual size,
zoom preservation across tab hiding and file refresh, and error/retry feedback.
