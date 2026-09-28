import { defineConfig, devices } from "@playwright/test";

export default defineConfig({
  testDir: "./e2e",
  timeout: 30_000,
  retries: 0,
  webServer: {
    command: "node ./e2e/serve.mjs",
    url: "http://127.0.0.1:8799/healthz",
    timeout: 120_000,
    reuseExistingServer: false,
  },
  use: {
    baseURL: "http://127.0.0.1:8799",
    ...devices["Desktop Chrome"],
  },
  reporter: [["list"]],
});
