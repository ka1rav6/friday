import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// The dev server proxies to the Rust backend so the browser sees one origin
// and the WebSocket upgrade works without CORS or a hardcoded port.
export default defineConfig({
  plugins: [react()],
  server: {
    port: 5173,
    proxy: {
      "/api": { target: "http://127.0.0.1:8787", changeOrigin: true, ws: true },
    },
  },
});
