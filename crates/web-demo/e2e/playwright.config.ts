import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "./tests",
  fullyParallel: true,
  reporter: "list",
  use: {
    baseURL: "http://127.0.0.1:4173",
    trace: "retain-on-failure",
  },
  webServer: {
    command: "python3 -m http.server 4173 --directory ../dist",
    url: "http://127.0.0.1:4173/index.html",
    reuseExistingServer: !process.env.CI,
  },
});
