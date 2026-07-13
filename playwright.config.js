const { defineConfig } = require("@playwright/test");

module.exports = defineConfig({
  testDir: "./tests/e2e",
  timeout: 30_000,
  retries: process.env.CI ? 2 : 0,
  use: {
    baseURL: process.env.BASE_URL || "http://127.0.0.1:8080",
    trace: "retain-on-failure",
  },
  webServer: process.env.BASE_URL ? undefined : {
    command: "cargo run -p mayab-cli --bin mayab-arbitrage",
    url: "http://127.0.0.1:8080/healthz",
    timeout: 120_000,
    reuseExistingServer: !process.env.CI,
    env: {
      ...process.env,
      RUST_LOG: "error",
      AUDITORIA_DB_PATH: "/tmp/mayab-playwright.sqlite",
      DEMO_RENTABLE_INICIAL: "false",
      ENABLED_EXCHANGES: "Binance,Kraken",
      // Una suite completa abre el dashboard muchas veces desde la misma IP.
      // Conservamos el rate limit de producción y ampliamos únicamente el
      // presupuesto del servidor efímero de Playwright.
      HTTP_READ_RPM: "10000",
    },
  },
});
