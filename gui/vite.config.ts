import { defineConfig } from "vite";

export default defineConfig({
  server: {
    port: 5317,
    strictPort: true,
  },
  // JavaScript is written down to what older system browsers read: a
  // phone's WebView can be years behind (Chromium 83 showed a blank page
  // when the build kept ES2022 syntax). Styles that such a browser does not
  // know fall back, and the interface says so instead of going blank.
  build: {
    target: ["es2020", "chrome80"],
  },
});
