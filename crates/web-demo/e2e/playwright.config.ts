import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "./tests",
  fullyParallel: true,
  reporter: "list",
  use: {
    baseURL: "http://127.0.0.1:4173/televim/",
    trace: "retain-on-failure",
  },
  webServer: {
    command: "mkdir -p ../../../target/web-demo-site && ln -sfn \"$(pwd)/../dist\" ../../../target/web-demo-site/televim && python3 -m http.server 4173 --directory ../../../target/web-demo-site",
    url: "http://127.0.0.1:4173/televim/index.html",
    reuseExistingServer: !process.env.CI,
  },
});
