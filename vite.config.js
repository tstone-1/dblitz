import { defineConfig } from "vite";
import adapter from "@sveltejs/adapter-static";
import { sveltekit } from "@sveltejs/kit/vite";
import { vitePreprocess } from "@sveltejs/vite-plugin-svelte";

const host = process.env.TAURI_DEV_HOST;

// https://vitejs.dev/config/
export default defineConfig(async () => ({
  // SvelteKit 3 takes its configuration here; it refuses a svelte.config.js.
  // Tauri has no Node.js server for SSR, so adapter-static prerenders the app
  // (SSG). See https://v2.tauri.app/start/frontend/sveltekit/
  plugins: [sveltekit({ adapter: adapter(), preprocess: vitePreprocess() })],
  test: {
    environment: "node",
    include: ["src/**/*.test.ts"],
    // Persist transformed modules across runs and across separate vitest
    // processes. Transform is roughly half of a cold run here, and the cache
    // lives in node_modules/.vitest-cache, so reinstalling deps invalidates it.
    fsModuleCache: true,
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));
