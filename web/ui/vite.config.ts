import { defineConfig } from "vite";
import solid from "vite-plugin-solid";

export default defineConfig({
  plugins: [solid()],
  base: "./",
  build: {
    target: "safari18",
    outDir: "dist",
    emptyOutDir: true,
    // One file each: the host serves them from disk, no network to split for.
    modulePreload: false,
    cssCodeSplit: false,
  },
});
