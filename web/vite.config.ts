import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig, loadEnv } from "vite";
import { mockApi, type MockMode } from "./mock/plugin.ts";

// The Rust server (crates/pictesa-server) serves dist/ and the API on one origin. In development
// the API calls go to it through the proxy, or to the built-in mock with VITE_MOCK=1 / hosted.
export default defineConfig(({ command, mode }) => {
  const env = loadEnv(mode, process.cwd(), "");
  const target = env.PICTESA_SERVER || "http://127.0.0.1:4747";
  const mock = command === "serve" ? env.VITE_MOCK : undefined;
  // changeOrigin sends Host as the server expects (it refuses non-loopback hosts). Non-GET calls
  // also need the dev origin in PICTESA_ALLOWED_HOSTS on the server, see README.md.
  const proxy = Object.fromEntries(["/api", "/files", "/thumbs", "/mcp"].map((path) => [path, { target, changeOrigin: true }]));

  return {
    plugins: [react(), tailwindcss(), ...(mock ? [mockApi({ mode: (["hosted", "token"].includes(mock) ? mock : "local") as MockMode })] : [])],
    server: {
      port: Number(env.PORT) || 5173,
      proxy: mock ? undefined : proxy,
    },
    preview: { proxy },
    build: {
      outDir: "dist",
      emptyOutDir: true,
      target: "es2022",
      cssCodeSplit: true,
      sourcemap: false,
    },
  };
});
