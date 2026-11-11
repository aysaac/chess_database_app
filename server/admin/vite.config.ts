import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// Built into dist/, which server/build.rs embeds in the chess-server binary.
// `npm run dev` serves the UI on 5175 and passes /api to a server on 8080
// (CHESS_SERVER_URL to change it).
export default defineConfig({
  plugins: [svelte()],
  clearScreen: false,
  server: {
    port: 5175,
    strictPort: true,
    host: "127.0.0.1",
    proxy: { "/api": process.env.CHESS_SERVER_URL ?? "http://127.0.0.1:8080" },
  },
  build: {
    target: "es2022",
    // No inline script: the server's Content-Security-Policy allows none.
    modulePreload: { polyfill: false },
  },
});
