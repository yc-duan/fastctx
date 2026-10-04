import { defineConfig } from "vite";

export default defineConfig({
  server: {
    port: 5317,
    strictPort: true,
  },
  build: {
    target: "es2022",
  },
});
