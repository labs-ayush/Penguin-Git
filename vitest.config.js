import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  test: {
    environment: "jsdom",
    setupFiles: ["./src/test/setup.js"],
    exclude: ["node_modules", "**/dist/**", "src-tauri", ".claude", "extensions/**"],
  },
});
