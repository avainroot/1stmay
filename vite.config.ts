import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  clearScreen: false,
  resolve: {
    dedupe: ["react", "react-dom"],
  },
  server: {
    strictPort: true,
    host: "127.0.0.1",
    port: 1420,
  },
  envPrefix: ["VITE_", "TAURI_ENV_*"],
  build: {
    target: process.env.TAURI_ENV_PLATFORM === "windows" ? "chrome105" : "safari13",
    minify: !process.env.TAURI_ENV_DEBUG ? "esbuild" : false,
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
    rollupOptions: {
      output: {
        manualChunks(id) {
          if (id.includes("/node_modules/.pnpm/three@")) return "three";
          if (
            id.includes("/node_modules/.pnpm/skinview-utils@") ||
            id.includes("/node_modules/.pnpm/skinview3d@")
          ) {
            return "skinview3d";
          }
          return undefined;
        },
      },
    },
  },
});
