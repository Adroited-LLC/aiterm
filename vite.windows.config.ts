import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  root: "windows-ui",
  build: { outDir: "../dist-windows", emptyOutDir: true },
});
