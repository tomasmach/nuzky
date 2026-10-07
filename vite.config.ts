import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// Tauri expects a fixed port and fails if it is taken instead of silently moving.
export default defineConfig({
  plugins: [react(), tailwindcss()],
  clearScreen: false,
  server: { port: 1420, strictPort: true, host: "localhost", watch: { ignored: ["**/src-tauri/**", "**/crates/**", "**/docs/**", "**/skills/**", "**/scripts/**", "**/target/**", "**/target-*/**", "**/tmp-test/**"] } },
  build: { target: "es2022" },
});
