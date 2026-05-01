import { defineConfig, type Plugin } from "vite";
import { copyFileSync, mkdirSync } from "node:fs";
import { resolve } from "node:path";

const host = process.env.TAURI_DEV_HOST;

const copyGhCss = (): Plugin => {
  const copy = () => {
    mkdirSync("public", { recursive: true });
    const pkg = "node_modules/github-markdown-css";
    copyFileSync(resolve(`${pkg}/github-markdown-light.css`), "public/gh-light.css");
    copyFileSync(resolve(`${pkg}/github-markdown-dark.css`), "public/gh-dark.css");
  };
  return {
    name: "copy-gh-css",
    configResolved: copy,
  };
};

export default defineConfig({
  plugins: [copyGhCss()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? { protocol: "ws", host, port: 1421 }
      : undefined,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  envPrefix: ["VITE_", "TAURI_ENV_*"],
  build: {
    target:
      process.env.TAURI_ENV_PLATFORM === "windows" ? "chrome105" : "safari13",
    minify: process.env.TAURI_ENV_DEBUG ? false : "esbuild",
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
  },
});
