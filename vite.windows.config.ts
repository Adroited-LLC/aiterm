import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  define: { "import.meta.env.VITE_WINDOWS_WSL": JSON.stringify("1") },
  root: "windows-ui",
  build: { outDir: "../dist-windows", emptyOutDir: true },
});
