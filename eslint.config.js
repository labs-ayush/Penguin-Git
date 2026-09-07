import js from "@eslint/js";
import reactHooks from "eslint-plugin-react-hooks";
import reactRefresh from "eslint-plugin-react-refresh";
import globals from "globals";

export default [
  { ignores: ["dist/**", "**/dist/**", "extensions/**", "src-tauri/**", "node_modules/**", "target/**"] },
  js.configs.recommended,
  {
    files: ["**/*.{js,jsx}"],
    languageOptions: {
      ecmaVersion: 2022,
      sourceType: "module",
      parserOptions: { ecmaFeatures: { jsx: true } },
      globals: { ...globals.browser, ...globals.node },
    },
    plugins: {
      "react-hooks": reactHooks,
      "react-refresh": reactRefresh,
    },
    rules: {
      // Kept at the plugin's recommended severities. The pre-rewrite prototype needed
      // `react-hooks/set-state-in-effect` downgraded to a warning to keep CI green; this
      // rebuild starts with no legacy components, so the rule stays at full strength.
      ...reactHooks.configs.recommended.rules,
      "react-refresh/only-export-components": ["warn", { allowConstantExport: true }],
      "no-unused-vars": ["warn", { argsIgnorePattern: "^_" }],
    },
  },
  {
    files: ["**/*.test.{js,jsx}", "src/test/**"],
    languageOptions: {
      globals: { ...globals.node, ...globals.browser },
    },
  },
];
