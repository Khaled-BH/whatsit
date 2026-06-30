import { defineConfig } from "vite";

// Vite config tuned for Tauri: fixed dev port, no screen clearing.
export default defineConfig({
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
  build: {
    target: "es2021",
    minify: true,
  },
});
