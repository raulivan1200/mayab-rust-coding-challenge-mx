const { test, expect } = require("@playwright/test");

test("dashboard carga sin errores ni logs de debug por defecto", async ({ page }) => {
  const errors = [];
  const logs = [];
  page.on("pageerror", error => errors.push(error.message));
  page.on("console", message => {
    if (message.type() === "error") errors.push(message.text());
    if (message.type() === "log") logs.push(message.text());
  });

  await page.goto("/");
  await expect(page.locator("#pnl")).toBeVisible();
  await expect(page.locator("#balances")).toBeAttached();
  await expect(page.locator(".landing-stack-phrase")).toContainText("Rust en el camino crítico");
  expect(errors).toEqual([]);
  expect(logs).toEqual([]);
});

test("salud, readiness y caching exponen contratos operativos", async ({ request }) => {
  const health = await request.get("/healthz");
  expect(health.ok()).toBeTruthy();
  expect(await health.json()).toMatchObject({ ok: true });
  expect(health.headers()["cache-control"]).toBe("no-store");

  const ready = await request.get("/readyz");
  expect([200, 503]).toContain(ready.status());
  const body = await ready.json();
  expect(typeof body.ready).toBe("boolean");
  expect(Array.isArray(body.checks)).toBeTruthy();

  const html = await request.get("/");
  expect(html.headers()["cache-control"]).toContain("no-cache");
  const asset = await request.get("/styles.css");
  expect(asset.headers()["cache-control"]).toContain("max-age=3600");
});

test("selector superior separa mercado, replay, demo y escala del corpus", async ({ page }) => {
  await page.route("**/api/research/tapes", route => route.fulfill({
    status: 200,
    contentType: "application/json",
    body: JSON.stringify({
      corpus: {
        totalEvents: 1_250_000,
        uniqueTapes: 48,
        totalCaptureDurationMs: 90_000_000,
        corpusSha256: "fixture-corpus-sha",
        evidenceGates: { publishableScale: true },
      },
      scanStatus: "matched_corpus",
      quantitativeScan: {
        netDislocations: 12_345,
        grossRate95: { perMillion: 20_000, lowerPerMillion95: 19_650, upperPerMillion95: 20_355 },
        netRate95: { perMillion: 9_876, lowerPerMillion95: 9_605, upperPerMillion95: 10_154 },
        liquidNetRate95: { perMillion: 8_765, lowerPerMillion95: 8_510, upperPerMillion95: 9_027 },
      },
    }),
  }));
  await page.goto("/");
  await expect(page.locator('[data-data-lens="live"]')).toContainText("Mercado");
  await expect(page.locator('[data-data-lens="replay"]')).toHaveAttribute("href", "/replay/");
  await expect(page.locator('[data-data-lens="demo"]')).toContainText("Demo");
  await expect(page.locator("#dataLensScale")).toHaveAttribute("data-status", "verified");
  await expect(page.locator("#dataLensScale")).toContainText("12,345 netas");
  await expect(page.locator("#dataLensScale")).toHaveAttribute("title", /IC Wilson 95%.*netas con liquidez/);
});

test("demo rentable mantiene PnL positivo y GA activo", async ({ request }) => {
  // La demo también evoluciona el GA; un build debug frío puede tardar más que
  // el timeout general de UI aunque el binario release responda en segundos.
  test.setTimeout(120_000);
  const response = await request.post("/api/demo", {
    data: { escenario: "mercado_rentable" },
  });
  expect(response.ok()).toBeTruthy();
  const state = await (await request.get("/api/estado")).json();
  expect(state.metricas.utilidadAcumuladaUsd).toBeGreaterThan(0);
  expect(state.operaciones.length).toBeGreaterThan(0);
  expect(state.genetico?.activo).toBeTruthy();
});
