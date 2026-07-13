const { test, expect } = require("@playwright/test");

async function mantenerWebSocketSilencioso(page) {
  await page.routeWebSocket("**/tiempo-real", () => {});
}

function cotizacionFixture(exchange, recibidaEn, ultimoMensaje = "book_ticker") {
  return {
    exchange,
    par: "BTC/USD",
    bid: exchange === "Binance" ? 50_010 : 50_020,
    bidCantidad: 2,
    ask: exchange === "Binance" ? 50_012 : 50_022,
    askCantidad: 2,
    eventoUnixMs: Date.parse(recibidaEn),
    recibidaEn,
    latenciaMs: 12,
    secuencia: 1,
    integrityStatus: "valid",
    resyncs: 0,
    sequenceGaps: 0,
    checksumFailures: 0,
    invalidatedMs: 0,
    timestampConfiable: true,
    conectado: true,
    ultimoMensaje,
  };
}

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
  await expect(page.locator(".landing-stack-phrase")).toContainText("Rust lleva el pulso");
  // Cierra explícitamente el stream antes de que Playwright destruya el
  // contexto; el dashboard mantiene un WebSocket vivo por diseño.
  await page.close();
  expect(errors).toEqual([]);
  expect(logs).toEqual([]);
});

test("debug requiere exactamente debug=1", async ({ page }) => {
  const consoleMessages = [];
  page.on("console", message => {
    consoleMessages.push(`${message.type()}:${message.text()}`);
  });

  await page.goto("/?debug=0");
  await expect(page.locator("html")).not.toHaveAttribute("data-mayab-debug", "1");
  expect(await page.evaluate(() => window.mayabDebugMetrics)).toBeUndefined();
  expect(consoleMessages).toEqual([]);
});

test("controles usan relleno vertical sin salto escala ni sombra", async ({ page }) => {
  await page.goto("/");
  const cta = page.locator("#btnJuryProofHero");
  await cta.hover();
  await expect(cta).toHaveCSS("background-size", "100% 100%");
  await expect(cta).toHaveCSS("transform", "none");
  await expect(cta).toHaveCSS("box-shadow", "none");

  const tab = page.locator(".tab-btn").first();
  await tab.hover();
  await expect(tab).toHaveCSS("background-size", "100% 100%");
  await expect(tab).toHaveCSS("transform", "none");
  await expect(tab).toHaveCSS("box-shadow", "none");
});

test("replay y consola operativa cargan sin errores de navegador", async ({ page }) => {
  const errors = [];
  page.on("pageerror", error => errors.push(error.message));
  page.on("console", message => {
    if (message.type() === "error") errors.push(message.text());
  });

  await page.goto("/replay/");
  await expect(page.locator("#status")).toBeVisible();
  await expect(page.locator("#resultTitle")).toContainText("Todavía no hay replay");
  await expect(page.locator('[data-data-lens="replay"]')).toHaveAttribute("aria-current", "page");

  await page.goto("/operator");
  await expect(page.locator("#banner")).toBeVisible();
  await expect(page.locator("#risk")).toBeAttached();
  await expect(page.locator("#exchanges")).toBeAttached();

  expect(errors).toEqual([]);
});

test("replay carga diez minutos automáticamente y permite cambiar la ventana", async ({ page }) => {
  let selectedSnapshots = 0;
  const requestedWindows = [];
  await page.route("**/api/replay/captura/estado", route => route.fulfill({
    status: 200,
    contentType: "application/json",
    body: JSON.stringify({
      activa: false,
      snapshots: selectedSnapshots,
      duracionSegundos: selectedSnapshots ? 300 : 0,
      historialSnapshots: 240,
      historialVentanaPredeterminadaSnapshots: 120,
      historialVentanaPredeterminadaDuracionSegundos: 600,
      historialDesde: "2026-07-12T18:00:00Z",
      historialHasta: "2026-07-12T18:10:00Z",
    }),
  }));
  await page.route("**/api/replay/captura/ventana", async route => {
    const payload = route.request().postDataJSON();
    requestedWindows.push(payload.minutos);
    selectedSnapshots = payload.minutos === 5 ? 60 : 120;
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ ok: true, snapshots: selectedSnapshots }),
    });
  });

  await page.goto("/replay/");
  await expect.poll(() => requestedWindows).toEqual([10]);
  await expect(page.locator("#status")).toContainText("Ventana seleccionada lista");

  await page.locator("#windowMinutes").selectOption("5");
  await page.locator("#loadWindow").click();
  await expect.poll(() => requestedWindows).toEqual([10, 5]);
  await expect(page.locator("#snapshots")).toHaveText("60");
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

test("el badge LIVE exige al menos una cotización WebSocket fresca", async ({ page }) => {
  await mantenerWebSocketSilencioso(page);
  let cotizacionesFrescas = false;

  await page.route("**/api/estado", async route => {
    const response = await route.fetch();
    const body = await response.json();
    const generadoEn = new Date();
    const staleMs = 1_000;
    const recibidaEn = new Date(
      generadoEn.getTime() - (cotizacionesFrescas ? 50 : staleMs + 5_000),
    ).toISOString();
    body.generadoEn = generadoEn.toISOString();
    body.configuracion = { ...body.configuracion, staleMs };
    body.exchangesActivos = { Binance: true, Kraken: true };
    body.cotizaciones = [
      cotizacionFixture("Binance", recibidaEn),
      cotizacionFixture("Kraken", recibidaEn),
    ];
    body.eventosEjecucion = [];
    body.metricas = { ...body.metricas, utilidadAcumuladaUsd: 0 };
    await route.fulfill({ response, body: JSON.stringify(body) });
  });

  await page.goto("/");
  await expect(page.locator("#juryProofMarket")).toContainText("0 WS frescos");
  await expect(page.locator("#modoOperacionBadge")).toHaveText("WS STALE");
  await expect(page.locator("#modoOperacionBadge")).not.toHaveText("LIVE WS");

  cotizacionesFrescas = true;
  await page.reload();
  await expect(page.locator("#juryProofMarket")).toContainText("2 WS frescos");
  await expect(page.locator("#modoOperacionBadge")).toHaveText("LIVE WS");
});

test("al cambiar entre mercado y demo los datos permanecen visibles sin reveal", async ({ page }) => {
  await page.goto("/");
  const revealCard = page.locator(".landing-grid .landing-card.reveal-card").first();
  await revealCard.scrollIntoViewIfNeeded();
  await expect(revealCard).toHaveClass(/is-visible/);

  await page.locator('[data-data-lens="live"]').click();
  await expect(page.locator("#tab-mercado")).toHaveClass(/activo/);
  await expect(page.locator("#tab-mercado .panel").first()).not.toHaveClass(/reveal-card/);
  await expect(page.locator("#tab-mercado .panel").first()).toHaveCSS("opacity", "1");

  await page.locator('[data-data-lens="demo"]').click();
  await expect(page.locator("#tab-riesgo")).toHaveClass(/activo/);
  await expect(page.locator("#tab-riesgo .panel").first()).not.toHaveClass(/reveal-card/);
  await expect(page.locator("#tab-riesgo .panel").first()).toHaveCSS("opacity", "1");

  await expect(page.locator(".metricas article").first()).not.toHaveClass(/reveal-card/);
  await expect(page.locator(".llm-strip")).not.toHaveClass(/reveal-card/);
  await expect(page.locator(".tabs-nav")).not.toHaveClass(/reveal-card/);
  await expect(revealCard).toHaveClass(/is-visible/);
  await page.close();
});

test("las seis pruebas aceptan clic en su texto y preparan evidencia dentro del dashboard", async ({ page }) => {
  let demoFinalCalls = 0;
  await page.route("**/api/**", route => {
    const path = new URL(route.request().url()).pathname;
    if (path === "/api/demo/final") {
      demoFinalCalls += 1;
      return route.fulfill({
        status: 200,
        contentType: "application/json; charset=utf-8",
        body: JSON.stringify({
          ok: true,
          corridaId: "jury-navigation-test",
          metricas: { utilidadAcumuladaUsd: 1 },
          riesgoSegundaPierna: { estadoFinal: "conciliada" },
          mlEdge: { version: "test" },
          preflight: { judgeReadiness: { status: "ready", passed: 12, total: 12 } },
        }),
      });
    }
    return route.fulfill({
      status: 200,
      contentType: "application/json; charset=utf-8",
      body: "{}",
    });
  });
  await page.goto("/");

  await page.locator('[data-jury-proof="market"] strong').click();
  await expect(page.locator("#tab-mercado")).toHaveClass(/activo/);

  await page.locator('[data-jury-proof="wallets"] small').click();
  await expect(page.locator("#tab-riesgo")).toHaveClass(/activo/);

  await page.locator('[data-jury-proof="economics"] strong').click();
  await expect(page.locator("#tab-evidence")).toHaveClass(/activo/);
  await expect.poll(() => demoFinalCalls).toBe(1);
  await expect(page.locator("#juryMinuteStatus")).toContainText("12/12 checks verdes");

  await expect(page.locator('[data-jury-proof="download"]')).toHaveAttribute("download", "");
});

test("la prueba de un clic no pinta éxito con una respuesta casi lista", async ({ page }) => {
  const respuestasInvalidas = [
    { ok: false, status: "ready", passed: 12, total: 12 },
    { ok: true, status: "blocked", passed: 12, total: 12 },
    { ok: true, status: "ready", passed: 11, total: 12 },
  ];
  let llamada = 0;
  await page.route("**/api/demo/final", route => {
    const caso = respuestasInvalidas[llamada++];
    return route.fulfill({
      status: 200,
      contentType: "application/json; charset=utf-8",
      body: JSON.stringify({
        ok: caso.ok,
        corridaId: `jury-blocked-${llamada}`,
        metricas: { utilidadAcumuladaUsd: 10 },
        riesgoSegundaPierna: { estadoFinal: "conciliada" },
        mlEdge: { version: "test" },
        preflight: {
          judgeReadiness: {
            status: caso.status,
            passed: caso.passed,
            total: caso.total,
          },
        },
      }),
    });
  });

  await page.goto("/");
  const status = page.locator("#juryMinuteStatus");
  for (const _ of respuestasInvalidas) {
    await page.locator("#btnJuryProofHero").click();
    await expect(status).toContainText("BLOCKED");
    await expect(status).not.toHaveClass(/\bok\b/);
    await expect(page.locator("#demoEstado")).toHaveText("evidencia bloqueada");
    await expect(page.locator("#demoFeedback")).toContainText("incompleta");
    await expect(page.locator("#demoFeedback")).toHaveAttribute("style", /var\(--rojo\)/);
    await expect(page.locator("#btnJuryProofHero")).toBeEnabled();
  }
  expect(llamada).toBe(respuestasInvalidas.length);
});

test("Evidence Lab no aprueba un ledger ausente", async ({ page }) => {
  await page.route("**/api/research/**", route => {
    const path = new URL(route.request().url()).pathname;
    if (path === "/api/research/ledger-audit") {
      return route.fulfill({
        status: 503,
        contentType: "application/json; charset=utf-8",
        body: JSON.stringify({ error: "ledger unavailable" }),
      });
    }
    return route.fulfill({
      status: 200,
      contentType: "application/json; charset=utf-8",
      body: "{}",
    });
  });
  await page.route("**/api/readiness/live", route => route.fulfill({
    status: 200,
    contentType: "application/json; charset=utf-8",
    body: "{}",
  }));

  await page.goto("/");
  await page.locator('.tab-btn[data-tab="tab-evidence"]').click();
  await expect(page.locator("#evidenceStatus")).toContainText("9 de 10 contratos respondieron");
  const ledger = page.locator(".evidence-lab-card").filter({
    hasText: "CORE · Auditoría del ledger",
  });
  await expect(ledger.locator("strong")).toHaveText("Revisión requerida");
  await expect(ledger.locator("code")).toHaveText("sin hash");
  await expect(ledger).not.toContainText("Checks del snapshot pasan");
});

test("diccionario y tutorial confinan el foco y vuelven inerte el fondo", async ({ page }) => {
  await page.emulateMedia({ reducedMotion: "reduce" });
  await page.goto("/");

  const fondo = page.locator(".pantalla");
  const dictionary = page.locator("#dictionaryPanel");
  const dictionaryClose = page.locator("#dictionaryClose");
  await page.locator("#dictionaryToggle").click();
  await expect(dictionary).toBeVisible();
  await expect(page.locator("#dictionarySearch")).toBeFocused();
  await expect.poll(() => fondo.evaluate(elemento => elemento.inert)).toBe(true);

  const ultimoDictionary = dictionary.locator([
    "a[href]",
    "button:not([disabled])",
    "input:not([disabled])",
    "select:not([disabled])",
    "textarea:not([disabled])",
    "[tabindex]:not([tabindex='-1'])",
  ].join(",")).last();
  await ultimoDictionary.focus();
  await page.keyboard.press("Tab");
  await expect(dictionaryClose).toBeFocused();
  await page.keyboard.press("Shift+Tab");
  await expect(ultimoDictionary).toBeFocused();
  await page.evaluate(() => document.getElementById("btnJuryProofHero").focus());
  await expect.poll(() => page.evaluate(() => (
    document.getElementById("dictionaryPanel").contains(document.activeElement)
  ))).toBe(true);

  await page.keyboard.press("Escape");
  await expect(dictionary).toBeHidden();
  await expect(page.locator("#dictionaryToggle")).toBeFocused();
  await expect.poll(() => fondo.evaluate(elemento => elemento.inert)).toBe(false);

  const tutorial = page.locator("#tutorialPanel");
  const tutorialClose = page.locator("#tutorialClose");
  const tutorialNext = page.locator("#tutorialNext");
  await page.locator("#tutorialToggle").click();
  await expect(tutorial).toBeVisible();
  await expect(tutorialNext).toBeFocused();
  await expect.poll(() => fondo.evaluate(elemento => elemento.inert)).toBe(true);
  await page.keyboard.press("Tab");
  await expect(tutorialClose).toBeFocused();
  await page.keyboard.press("Shift+Tab");
  await expect(tutorialNext).toBeFocused();

  await page.keyboard.press("Escape");
  await expect(tutorial).toBeHidden();
  await expect(page.locator("#tutorialToggle")).toBeFocused();
  await expect.poll(() => fondo.evaluate(elemento => elemento.inert)).toBe(false);
});

test("el header y su grid conservan su tamaño al hacer scroll", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  // Esta prueba valida geometría, no animación. Reducir movimiento evita que
  // los reveals y el canvas compitan con las mediciones de layout en CI.
  await page.emulateMedia({ reducedMotion: "reduce" });
  await page.goto("/");

  const pantalla = page.locator(".pantalla");
  const header = page.locator("#dashboard");
  const grid = page.locator("#header-grid");
  await header.scrollIntoViewIfNeeded();
  await page.evaluate(() => document.fonts.ready);

  await expect(header).not.toHaveClass(/reveal-card/);
  await expect(header).toHaveCSS("transform", "none");
  const antes = await Promise.all([header.boundingBox(), grid.boundingBox()]);
  await pantalla.evaluate((elemento) => elemento.scrollBy(0, 280));
  await page.waitForTimeout(250);
  const despues = await Promise.all([header.boundingBox(), grid.boundingBox()]);

  expect(antes[0]?.height).toBeGreaterThanOrEqual(310);
  expect(despues[0]?.height).toBe(antes[0]?.height);
  expect(despues[1]?.height).toBe(antes[1]?.height);
  await page.close();
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

test("prueba completa deja preflight 12 de 12 y evidencia económica", async ({ page, request }) => {
  test.setTimeout(120_000);
  await page.goto("/");
  await expect(page.locator("#juryMinute li")).toHaveCount(6);
  await expect(page.locator("#btnJuryProofHero")).toContainText("Pon a Mayab contra las cuerdas");
  await page.locator("#btnJuryProofHero").click();
  await expect(page.locator("#juryMinuteStatus")).toContainText("12/12 checks verdes", { timeout: 120_000 });

  const preflight = await (await request.get("/api/preflight")).json();
  expect(preflight).toMatchObject({ listo: true, modo: "ready" });
  expect(preflight.judgeReadiness).toMatchObject({ status: "ready", passed: 12, total: 12 });
  expect(preflight.judgeReadiness.checks.every(check => check.ok === true)).toBeTruthy();
  expect(preflight.judgeReadiness.twoLegEvidence.invariants.allPassed).toBeTruthy();

  const economics = await (await request.get("/api/research/economics")).json();
  expect(economics.available).toBeTruthy();
  expect(economics.edgeWaterfall.items.length).toBeGreaterThanOrEqual(7);
  expect(economics.capacityCurve.points.length).toBeGreaterThanOrEqual(6);
  expect(economics.decisionFunnel.stages.length).toBeGreaterThanOrEqual(5);

  const matrix = await (await request.get("/api/research/execution-matrix")).json();
  expect(matrix).toMatchObject({ available: true, total: 12, passed: 12, allPassed: true });

  const version = await (await request.get("/api/version")).json();
  expect(version.schemaVersion).toBeTruthy();
  expect(version.evidenceSessionId).toMatch(/^jury-/);
  expect(version.datasetHash).toMatch(/^sha256:/);
  expect(version.configHash).toMatch(/^sha256:/);
});
