let ultimoEstado = null;
let tieneCambios = false;
let oportunidadSeleccionadaId = null;
const gaHistorial = [];
let gaAutoEnCurso = false;
const ID_PESTANA = `tab-${Date.now()}-${Math.random().toString(16).slice(2)}`;
const DEBUG_ACTIVO =
  new URLSearchParams(location.search).get("debug") === "1" ||
  localStorage.getItem("mayabDebug") === "1";
const INTERVALO_CANVAS_MS = 1000 / 30;
let ultimoFrameCanvas = 0;
const reducirMovimiento = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
const animacionGa = { firma: "", inicio: 0 };
const formatoHoraGrafica = new Intl.DateTimeFormat("es-MX", {
  hour: "2-digit",
  minute: "2-digit",
  second: "2-digit",
});

function formatearHoraLocal(valor) {
  const fecha = new Date(valor);
  return Number.isNaN(fecha.getTime()) ? "—" : formatoHoraGrafica.format(fecha);
}
let ultimoPreflightMs = 0;
let preflightCache = null;
let preflightEnCurso = false;
let wsReconnectMs = 600;
const WS_RECONNECT_MAX = 15_000;

// Fuente única para el copy editorial de la interfaz. Los estados que cambian
// en runtime también salen de aquí para evitar variantes dispersas.
const UI_COPY = Object.freeze({
  landingKicker: "Mercado en vivo · ejecución simulada",
  landingTitle: "Arbitraje BTC, explicado decisión por decisión.",
  landingBody: "Mayab compara precios entre exchanges y descuenta comisiones, slippage, latencia y liquidez. Solo acepta una ruta si la utilidad estimada supera el riesgo.",
  landingPrimaryCta: "Ver una prueba completa",
  landingSecondaryCta: "Abrir evidencia técnica",
  proofMarket: "Consultando cobertura de exchanges",
  proofCosts: "Costos incluidos en la utilidad neta",
  proofSafety: "Recuperación ante fallos de ejecución",
  socket: Object.freeze({
    connecting: "Conectando mercado",
    connected: "Canal conectado",
    realtime: "Tiempo real",
    reconnecting: "Recuperando señal",
    offline: "Sin conexión",
    stale: "Señal pausada",
    waiting: "esperando la primera señal",
    now: "datos actualizados ahora",
  }),
});

const metricasPrevias = {
  pnl: 0,
  retorno: 0,
  eventos: 0,
  latencia: 0,
  sharpe: 0,
  winRate: 0,
  maxDrawdown: 0,
  operacionesTotales: 0,
  operacionesFallidas: 0,
  rebalanceosTotales: 0,
};

const opsNotificadas = new Set();
const metricasDebug = DEBUG_ACTIVO ? crearDebugMetrics() : null;

const $ = (id) => document.getElementById(id);
const dinero = new Intl.NumberFormat("es-MX", {
  style: "currency",
  currency: "USD",
  maximumFractionDigits: 2,
});
const dineroMetrica = new Intl.NumberFormat("es-MX", {
  minimumFractionDigits: 2,
  maximumFractionDigits: 2,
});
const numero = new Intl.NumberFormat("es-MX", { maximumFractionDigits: 2 });
const btc = new Intl.NumberFormat("es-MX", { maximumFractionDigits: 6 });

function aplicarCopyEditorial() {
  document.querySelectorAll("[data-ui-copy]").forEach((el) => {
    const clave = el.dataset.uiCopy;
    if (typeof UI_COPY[clave] === "string") el.textContent = UI_COPY[clave];
  });
}

aplicarCopyEditorial();

let descargaEvidenciaEnCurso = null;

const MENSAJES_ESPERA_EVIDENCIA = Object.freeze([
  "Bitcoin no se apura; la evidencia tampoco se inventa.",
  "Contando sats. Sí, también los que se esconden en el sofá.",
  "Un momento: hasta Bitcoin espera confirmaciones.",
  "Sellando el SHA-256. El humo no cabe en el hash.",
  "Revisando dos veces; los sats no aceptan redondeos creativos.",
]);

function mensajeEsperaEvidencia(segundos) {
  const indice = Math.floor(segundos / 1.8) % MENSAJES_ESPERA_EVIDENCIA.length;
  return MENSAJES_ESPERA_EVIDENCIA[indice];
}

function iniciarDescargaEvidencia() {
  const cta = $("btnEvidenceHero");
  const label = cta?.querySelector(".evidence-download-label");
  const progress = cta?.querySelector(".evidence-download-track i");
  if (!cta || !label || !progress) return;

  const textoInicial = UI_COPY.landingSecondaryCta;
  const actualizar = (porcentaje, texto = "Descargando evidencia") => {
    const valor = Math.max(0, Math.min(100, Math.round(porcentaje)));
    progress.style.width = `${valor}%`;
    label.textContent = `${texto} · ${valor}%`;
    cta.setAttribute("aria-label", `${texto}: ${valor}%`);
    if (
      descargaEvidenciaEnCurso?.porcentaje &&
      descargaEvidenciaEnCurso?.barra &&
      descargaEvidenciaEnCurso?.estado
    ) {
      descargaEvidenciaEnCurso.porcentaje.textContent = `${valor}%`;
      descargaEvidenciaEnCurso.barra.style.width = `${valor}%`;
      descargaEvidenciaEnCurso.estado.textContent = texto;
    }
  };

  cta.addEventListener("click", async (event) => {
    event.preventDefault();
    if (descargaEvidenciaEnCurso) {
      descargaEvidenciaEnCurso.ventana?.focus();
      return;
    }

    const ventana = window.open("", "_blank");
    if (ventana) {
      ventana.opener = null;
      ventana.document.title = "Mayab · Preparando evidencia";
      ventana.document.body.innerHTML = `
        <main style="min-height:100vh;box-sizing:border-box;display:grid;place-content:center;gap:18px;padding:28px;background:#111;color:#f3f0e8;font:700 16px/1.35 Arial,sans-serif;text-align:center">
          <p style="margin:0;color:#ffb200;font-size:12px;letter-spacing:.14em;text-transform:uppercase">Mayab Arbitraje BTC</p>
          <strong id="mayabEvidencePercent" style="font-size:clamp(64px,18vw,180px);line-height:.8">0%</strong>
          <p id="mayabEvidenceStatus" style="margin:0;color:#a7a096">Preparando evidencia</p>
          <div style="width:min(520px,78vw);height:8px;overflow:hidden;border:1px solid #4a4a4a;background:#202020">
            <i id="mayabEvidenceBar" style="display:block;width:0;height:100%;background:#ffb200;transition:width 180ms ease"></i>
          </div>
        </main>`;
    }

    descargaEvidenciaEnCurso = {
      ventana,
      porcentaje: ventana?.document.getElementById("mayabEvidencePercent"),
      estado: ventana?.document.getElementById("mayabEvidenceStatus"),
      barra: ventana?.document.getElementById("mayabEvidenceBar"),
    };
    cta.dataset.loading = "true";
    cta.setAttribute("aria-busy", "true");
    actualizar(0, MENSAJES_ESPERA_EVIDENCIA[0]);
    const inicioPreparacion = Date.now();
    const pulsoPreparacion = window.setInterval(() => {
      const segundos = (Date.now() - inicioPreparacion) / 1000;
      const estimado = Math.min(89, 8 + (1 - Math.exp(-segundos / 12)) * 82);
      actualizar(estimado, mensajeEsperaEvidencia(segundos));
    }, 300);

    try {
      const response = await fetch(cta.href, { headers: { accept: "application/json" } });
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      if (!response.body) throw new Error("El navegador no permite leer el progreso");

      const total = Number(response.headers.get("x-mayab-content-length")) ||
        Number(response.headers.get("content-length"));
      const reader = response.body.getReader();
      const chunks = [];
      let recibidos = 0;

      while (true) {
        const { done, value } = await reader.read();
        if (done) break;
        window.clearInterval(pulsoPreparacion);
        chunks.push(value);
        recibidos += value.byteLength;
        const porcentajeDescarga = total > 0
          ? 90 + (recibidos / total) * 10
          : Math.min(99, 90 + chunks.length);
        actualizar(porcentajeDescarga);
      }

      actualizar(100, "Evidencia lista");
      const blobUrl = URL.createObjectURL(new Blob(chunks, { type: "application/json" }));
      if (ventana && !ventana.closed) {
        ventana.location.replace(blobUrl);
      } else {
        const enlace = document.createElement("a");
        enlace.href = blobUrl;
        enlace.target = "_blank";
        enlace.rel = "noopener";
        enlace.click();
      }
      window.setTimeout(() => URL.revokeObjectURL(blobUrl), 60_000);
    } catch (error) {
      label.textContent = "Reintentar evidencia";
      progress.style.width = "0%";
      if (ventana && !ventana.closed) {
        const estado = ventana.document.getElementById("mayabEvidenceStatus");
        if (estado) estado.textContent = "No se pudo cargar. Cierra esta pestaña y vuelve a intentar.";
      }
      debugWarn("No se pudo descargar el paquete de evaluación", error);
    } finally {
      window.clearInterval(pulsoPreparacion);
      cta.removeAttribute("aria-busy");
      cta.removeAttribute("aria-label");
      delete cta.dataset.loading;
      descargaEvidenciaEnCurso = null;
      window.setTimeout(() => {
        if (label.textContent === "Evidencia lista · 100%") label.textContent = textoInicial;
      }, 1200);
    }
  });
}

const DATA_LENS_COPY = Object.freeze({
  live: Object.freeze({
    eyebrow: "01 · Mercado en vivo",
    title: "Cotizaciones en vivo con ejecución simulada.",
    detail: "Las fuentes y los resultados se identifican por separado.",
    tab: "tab-mercado",
  }),
  replay: Object.freeze({
    eyebrow: "02 · Replay reproducible",
    title: "Repite una captura bajo las mismas condiciones.",
    detail: "El replay usa un estado aislado y no modifica la sesión en vivo.",
  }),
  demo: Object.freeze({
    eyebrow: "03 · Demo sintética",
    title: "Ejecuta escenarios controlados de operación y fallo.",
    detail: "Cada resultado de prueba queda identificado en la auditoría.",
    tab: "tab-riesgo",
  }),
});

function iniciarSelectorProcedencia() {
  const items = [...document.querySelectorAll("[data-data-lens]")];
  const eyebrow = $("dataLensEyebrow");
  const title = $("dataLensTitle");
  const detail = $("dataLensDetail");
  let lensSeleccionado = "live";

  const mostrar = (lens) => {
    const copy = DATA_LENS_COPY[lens];
    if (!copy) return;
    items.forEach((item) => {
      const activo = item.dataset.dataLens === lens;
      item.classList.toggle("is-active", activo);
      if (activo) item.setAttribute("aria-current", "page");
      else item.removeAttribute("aria-current");
    });
    if (eyebrow) eyebrow.textContent = copy.eyebrow;
    if (title) title.textContent = copy.title;
    if (detail) detail.textContent = copy.detail;
  };

  items.forEach((item) => {
    item.addEventListener("mouseenter", () => mostrar(item.dataset.dataLens));
    item.addEventListener("focus", () => mostrar(item.dataset.dataLens));
    item.addEventListener("mouseleave", () => mostrar(lensSeleccionado));
    item.addEventListener("blur", () => mostrar(lensSeleccionado));
    item.addEventListener("click", (event) => {
      const lens = item.dataset.dataLens;
      lensSeleccionado = lens;
      mostrar(lens);
      if (lens === "replay") return;
      event.preventDefault();
      const tab = document.querySelector(`[data-tab="${DATA_LENS_COPY[lens]?.tab}"]`);
      tab?.click();
      // La pestaña restaura su scroll en el siguiente frame. Esperar ese frame y
      // mover el contenedor principal evita dos animaciones simultáneas que, en
      // especial en Safari/touch, pueden dejar el gesto hacia arriba bloqueado.
      requestAnimationFrame(() => irAlDashboard(!reducirMovimiento));
    });
  });

  const lensInicial = new URLSearchParams(window.location.search).get("lens");
  if (lensInicial === "demo" || lensInicial === "live") {
    lensSeleccionado = lensInicial;
    mostrar(lensInicial);
    const tab = document.querySelector(`[data-tab="${DATA_LENS_COPY[lensInicial].tab}"]`);
    // The tab listeners are registered later in the same DOM-ready callback.
    queueMicrotask(() => tab?.click());
  }
}

async function cargarEscalaCorpusPublico() {
  const badge = $("dataLensScale");
  if (!badge) return;
  try {
    const response = await fetch("/api/research/tapes", {
      headers: { accept: "application/json" },
      cache: "no-store",
    });
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    const payload = await response.json();
    const corpus = payload?.corpus;
    const gates = corpus?.evidenceGates;
    const scan = payload?.scanStatus === "matched_corpus" ? payload?.quantitativeScan : null;
    if (!corpus || !gates) {
      badge.dataset.status = "unavailable";
      badge.textContent = "Corpus de mercado no publicado";
      badge.title = "La captura disponible todavía no cumple el umbral de publicación.";
      return;
    }
    const events = Number(corpus.totalEvents || 0);
    const shards = Number(corpus.uniqueTapes || 0);
    const hours = Number(corpus.totalCaptureDurationMs || 0) / 3_600_000;
    const verified = gates.publishableScale === true;
    const netDislocations = Number(scan?.netDislocations || 0);
    const formatWilson = (label, estimate) => {
      if (!estimate) return `${label}: sin intervalo`;
      const point = Number(estimate.perMillion || 0);
      const lower = Number(estimate.lowerPerMillion95 || 0);
      const upper = Number(estimate.upperPerMillion95 || 0);
      return `${label}: ${numero.format(point)} por millón (IC Wilson 95% ${numero.format(lower)}–${numero.format(upper)})`;
    };
    badge.dataset.status = verified ? "verified" : "pending";
    badge.textContent = verified && scan
      ? `${numero.format(events)} eventos · ${numero.format(netDislocations)} netas`
      : verified
        ? `${numero.format(events)} eventos · escala verificada`
        : `${numero.format(events)} eventos · escala pendiente`;
    const scanDetail = scan
      ? `${numero.format(netDislocations)} dislocaciones netas · ${formatWilson("brutas", scan.grossRate95)} · ${formatWilson("netas", scan.netRate95)} · ${formatWilson("netas con liquidez", scan.liquidNetRate95)} · ${numero.format(Number(scan.eventsPerSecond || 0))} eventos/s · ${numero.format(Number(scan.maxLevelsInMemory || 0))} niveles máximos residentes`
      : "scan cuantitativo pendiente";
    badge.title = `${shards} shards únicos · ${numero.format(hours)} h capturadas · ${scanDetail} · corpus ${corpus.corpusSha256 || "sin hash"}`;
  } catch (_) {
    badge.dataset.status = "unavailable";
    badge.textContent = "Corpus de mercado no disponible";
    badge.title = "No se pudo consultar /api/research/tapes.";
  }
}

function mostrarFeedback(el, mensaje, ok = true) {
  if (!el) return;
  el.textContent = mensaje;
  el.style.color = ok ? "var(--verde)" : "var(--rojo)";
}

async function mensajeErrorApi(res, fallback) {
  try {
    const body = await res.clone().json();
    return body?.error?.message || body?.message || fallback;
  } catch (_) {
    return fallback;
  }
}

function marcarCambio(el) {
  if (!el) return;
  el.classList.remove("pulse-verde", "ga-flash");
  void el.offsetWidth;
  el.classList.add("pulse-verde", "ga-flash");
}

function crearDebugMetrics() {
  return {
    inicio: performance.now(),
    wsMensajes: 0,
    wsBytes: 0,
    renders: 0,
    framesCanvas: 0,
    longTasks: 0,
    medidas: new Map(),
  };
}

function debugNow() {
  return DEBUG_ACTIVO ? performance.now() : 0;
}

function debugMeasure(nombre, inicio) {
  if (!DEBUG_ACTIVO || !inicio) return;
  const duracion = performance.now() - inicio;
  const actual = metricasDebug.medidas.get(nombre) || { n: 0, total: 0, max: 0 };
  actual.n += 1;
  actual.total += duracion;
  actual.max = Math.max(actual.max, duracion);
  metricasDebug.medidas.set(nombre, actual);
}

function debugLog(...args) {
  if (DEBUG_ACTIVO) console.debug("[mayab-debug]", ...args.map(debugSerialize));
}

function debugWarn(...args) {
  if (DEBUG_ACTIVO) console.warn("[mayab-debug]", ...args.map(debugSerialize));
}

function debugError(...args) {
  if (DEBUG_ACTIVO) console.error("[mayab-debug]", ...args.map(debugSerialize));
}

function debugSerialize(valor) {
  if (valor instanceof Error) return `${valor.name}: ${valor.message}`;
  if (typeof valor === "object" && valor !== null) {
    try {
      return JSON.stringify(valor);
    } catch (_) {
      return String(valor);
    }
  }
  return valor;
}

function textoCeldaTabla(celda) {
  return (celda?.innerText || celda?.textContent || "").replace(/\s+/g, " ").trim();
}

function escaparCsv(valor) {
  const texto = String(valor ?? "");
  return /[",\n\r]/.test(texto) ? `"${texto.replace(/"/g, '""')}"` : texto;
}

function nombreArchivoTabla(panel, indice) {
  const titulo = panel?.querySelector("h2")?.textContent?.trim() || `tabla-${indice + 1}`;
  const base = titulo
    .normalize("NFD")
    .replace(/[\u0300-\u036f]/g, "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-|-$/g, "");
  return `${base || `tabla-${indice + 1}`}-${new Date().toISOString().slice(0, 10)}.csv`;
}

function aplicarFiltroTabla(tabla) {
  const toolbar = tabla.closest(".panel, section")?.querySelector(`.tabla-herramientas[data-tabla-id="${tabla.dataset.tablaId}"]`);
  const input = toolbar?.querySelector(".tabla-filtro-input");
  if (!input) return;

  const consulta = input.value.trim().toLocaleLowerCase("es-MX");
  let visibles = 0;
  tabla.querySelectorAll("tbody tr").forEach((fila) => {
    const coincide = !consulta || textoCeldaTabla(fila).toLocaleLowerCase("es-MX").includes(consulta);
    fila.hidden = !coincide;
    if (coincide) visibles += 1;
  });

  const estado = toolbar.querySelector(".tabla-filtro-estado");
  if (estado) estado.textContent = consulta ? `${visibles} resultado${visibles === 1 ? "" : "s"}` : "";
}

function aplicarFiltrosTablas() {
  document.querySelectorAll("#dashboard .tabla-scroll > table[data-tabla-id]").forEach(aplicarFiltroTabla);
}

function descargarTablaCsv(tabla, panel, indice) {
  const encabezados = [...tabla.querySelectorAll("thead th")].map(textoCeldaTabla);
  const filas = [...tabla.querySelectorAll("tbody tr:not([hidden])")].map((fila) =>
    [...fila.querySelectorAll("th, td")].map(textoCeldaTabla),
  );
  const contenido = [encabezados, ...filas]
    .filter((fila) => fila.length)
    .map((fila) => fila.map(escaparCsv).join(","))
    .join("\r\n");
  const enlace = document.createElement("a");
  enlace.href = URL.createObjectURL(new Blob(["\ufeff", contenido], { type: "text/csv;charset=utf-8" }));
  enlace.download = nombreArchivoTabla(panel, indice);
  document.body.appendChild(enlace);
  enlace.click();
  enlace.remove();
  URL.revokeObjectURL(enlace.href);
}

async function alternarPantallaCompleta(panel) {
  if (panel.classList.contains("tabla-pantalla-completa")) {
    panel.classList.remove("tabla-pantalla-completa");
    document.body.classList.remove("tabla-fullscreen-activa");
    return;
  }
  if (document.fullscreenElement) {
    await document.exitFullscreen();
    return;
  }
  if (panel.requestFullscreen) {
    try {
      await panel.requestFullscreen();
      return;
    } catch (_) {
      // Safari/iOS y algunos webviews exponen la API pero rechazan la llamada.
      // En ese caso conservamos el fallback CSS y su salida por Escape/botón.
    }
  }
  panel.classList.add("tabla-pantalla-completa");
  document.body.classList.add("tabla-fullscreen-activa");
}

function iniciarHerramientasTablas() {
  const tablas = document.querySelectorAll("#dashboard .tabla-scroll > table");
  tablas.forEach((tabla, indice) => {
    if (tabla.dataset.tablaId) return;
    const scroll = tabla.parentElement;
    const panel = tabla.closest(".panel, section");
    if (!scroll || !panel) return;

    const id = `tabla-dashboard-${indice + 1}`;
    const titulo = panel.querySelector("h2")?.textContent?.trim() || `Tabla ${indice + 1}`;
    tabla.dataset.tablaId = id;
    const toolbar = document.createElement("div");
    toolbar.className = "tabla-herramientas";
    toolbar.dataset.tablaId = id;
    toolbar.setAttribute("aria-label", `Herramientas de ${titulo}`);
    toolbar.innerHTML = `
      <div class="tabla-filtro" hidden>
        <label class="sr-only" for="${id}-filtro">Filtrar ${escapeHtml(titulo)}</label>
        <input id="${id}-filtro" class="tabla-filtro-input" type="search" placeholder="Buscar en la tabla…" autocomplete="off" />
        <span class="tabla-filtro-estado" aria-live="polite"></span>
      </div>
      <div class="tabla-acciones">
        <button type="button" class="tabla-accion tabla-accion-filtro" aria-expanded="false" aria-controls="${id}-filtro" title="Filtrar filas">⌕ <span>Filtrar</span></button>
        <button type="button" class="tabla-accion tabla-accion-descarga" title="Descargar filas visibles como CSV">↓ <span>Descargar</span></button>
        <button type="button" class="tabla-accion tabla-accion-fullscreen" title="Ver tabla en pantalla completa"><span>Pantalla completa</span></button>
      </div>`;
    scroll.before(toolbar);

    const filtro = toolbar.querySelector(".tabla-filtro");
    const input = toolbar.querySelector(".tabla-filtro-input");
    const btnFiltro = toolbar.querySelector(".tabla-accion-filtro");
    btnFiltro.addEventListener("click", () => {
      const abrir = filtro.hidden;
      filtro.hidden = !abrir;
      btnFiltro.setAttribute("aria-expanded", String(abrir));
      btnFiltro.classList.toggle("activo", abrir);
      if (abrir) input.focus();
    });
    input.addEventListener("input", () => aplicarFiltroTabla(tabla));
    toolbar.querySelector(".tabla-accion-descarga").addEventListener("click", () => descargarTablaCsv(tabla, panel, indice));
    toolbar.querySelector(".tabla-accion-fullscreen").addEventListener("click", () => alternarPantallaCompleta(panel));
  });

  document.addEventListener("fullscreenchange", () => {
    document.querySelectorAll(".tabla-accion-fullscreen").forEach((boton) => {
      const activo = Boolean(document.fullscreenElement && document.fullscreenElement.contains(boton));
      boton.classList.toggle("activo", activo);
      boton.querySelector("span").textContent = activo ? "Salir" : "Pantalla completa";
      boton.title = activo ? "Salir de pantalla completa" : "Ver tabla en pantalla completa";
    });
  });
  document.addEventListener("keydown", (event) => {
    if (event.key !== "Escape") return;
    const panel = document.querySelector(".tabla-pantalla-completa");
    if (!panel) return;
    panel.classList.remove("tabla-pantalla-completa");
    document.body.classList.remove("tabla-fullscreen-activa");
  });
}

function iniciarDebug() {
  if (!DEBUG_ACTIVO) return;
  window.mayabDebugMetrics = metricasDebug;
  document.documentElement.dataset.mayabDebug = "1";
  debugLog("debug activo", { tab: ID_PESTANA });
  if ("PerformanceObserver" in window) {
    try {
      const observer = new PerformanceObserver((list) => {
        metricasDebug.longTasks += list.getEntries().length;
      });
      observer.observe({ type: "longtask", buffered: true });
    } catch (err) {
      debugWarn("longtask observer no disponible", err);
    }
  }
  setInterval(() => {
    const medidas = Object.fromEntries(
      [...metricasDebug.medidas.entries()].map(([nombre, m]) => [
        nombre,
        {
          n: m.n,
          avgMs: Number((m.total / Math.max(m.n, 1)).toFixed(2)),
          maxMs: Number(m.max.toFixed(2)),
        },
      ]),
    );
    debugLog("perf", {
      uptimeSeg: Number(((performance.now() - metricasDebug.inicio) / 1000).toFixed(1)),
      wsMensajes: metricasDebug.wsMensajes,
      wsKb: Number((metricasDebug.wsBytes / 1024).toFixed(1)),
      renders: metricasDebug.renders,
      framesCanvas: metricasDebug.framesCanvas,
      longTasks: metricasDebug.longTasks,
      medidas,
    });
  }, 5000);
}

iniciarDebug();
iniciarTema();
arrancar();
cargarConfigGa();
iniciarBacktest();
iniciarResearchLab();
iniciarEvidenceLab();
iniciarPresets();
iniciarDemo();

async function arrancar() {
  cargarVersion();
  try {
    const res = await fetch("/api/estado");
    if (res.ok) {
      const datos = await res.json();
      ultimoEstado = datos;
      estado.ultimoMensaje = Date.now();
      tieneCambios = true;
      detectarNotificaciones(datos);
      renderizar(ultimoEstado);
    }
  } catch (e) {
    debugError("No se pudo cargar estado inicial REST", e);
  }
  conectar();
}

async function cargarVersion() {
  try {
    const res = await fetch("/api/version");
    if (!res.ok) return;
    const version = await res.json();
    const link = $("buildVersionLink");
    if (!link) return;
    const sha = String(version.gitSha || "local");
    link.textContent = `${sha.slice(0, 7)} · ${version.environment || "development"}`;
    link.title = `Build ${version.version || ""} · ${version.buildTime || ""}`;
  } catch (error) {
    debugError("No se pudo cargar la versión desplegada", error);
  }
}
iniciarHeaderColapsable();
iniciarTutorial();
setInterval(verificarConexion, 900);
setInterval(actualizarTiempoSocket, 1000);

function loopAnimacion(timestamp) {
  if (tieneCambios && ultimoEstado) {
    const inicio = debugNow();
    renderizar(ultimoEstado);
    if (DEBUG_ACTIVO) metricasDebug.renders += 1;
    debugMeasure("render", inicio);
    tieneCambios = false;
  }
  if (ultimoEstado && timestamp - ultimoFrameCanvas >= INTERVALO_CANVAS_MS) {
    const inicio = debugNow();
    dibujarMapa(ultimoEstado);
    dibujarSeries(ultimoEstado, timestamp);
    dibujarGa(ultimoEstado.genetico);
    ultimoFrameCanvas = timestamp;
    if (DEBUG_ACTIVO) metricasDebug.framesCanvas += 1;
    debugMeasure("canvas", inicio);
  }
  requestAnimationFrame(loopAnimacion);
}
requestAnimationFrame(loopAnimacion);

async function conectar() {
  const protocolo = location.protocol === "https:" ? "wss" : "ws";
  const socket = new WebSocket(`${protocolo}://${location.host}/tiempo-real`);
  cambiarSocket(UI_COPY.socket.connecting);

  socket.addEventListener("open", () => {
    cambiarSocket(UI_COPY.socket.connected, true);
    wsReiniciarBackoff();
  });

  socket.addEventListener("message", (evento) => {
    try {
      const inicio = debugNow();
      const datos = JSON.parse(evento.data);
      if (DEBUG_ACTIVO) {
        metricasDebug.wsMensajes += 1;
        metricasDebug.wsBytes += evento.data.length || 0;
      }
      debugMeasure("ws-parse", inicio);
      wsReiniciarBackoff();
      ultimoEstado = datos;
      estado.ultimoMensaje = Date.now();
      cambiarSocket(UI_COPY.socket.realtime, true);
      tieneCambios = true;
      detectarNotificaciones(datos);
    } catch (err) {
      debugError("Error parseando WebSocket:", err);
    }
  });

  socket.addEventListener("close", () => {
    cambiarSocket(UI_COPY.socket.reconnecting);
    debugWarn("websocket cerrado; reconectando en %dms", wsReconnectMs);
    const ms = wsReconnectMs;
    wsReconnectMs = Math.min(Math.round(wsReconnectMs * 1.8), WS_RECONNECT_MAX);
    setTimeout(conectar, ms);
  });

  socket.addEventListener("error", (err) => {
    cambiarSocket(UI_COPY.socket.offline, false);
    debugError("error de websocket", err);
  });
}

function wsReiniciarBackoff() {
  wsReconnectMs = 600;
}

const estado = {
  ultimoMensaje: 0,
};

function verificarConexion() {
  if (!estado.ultimoMensaje) return;
  const viejo = Date.now() - estado.ultimoMensaje > 2400;
  if (viejo) cambiarSocket(UI_COPY.socket.stale, false);
}

function actualizarTiempoSocket() {
  const el = $("estadoSocketTiempo");
  if (!el) return;
  if (!estado.ultimoMensaje) {
    el.textContent = UI_COPY.socket.waiting;
    return;
  }
  const segundos = Math.max(0, Math.floor((Date.now() - estado.ultimoMensaje) / 1000));
  el.textContent = segundos < 2 ? UI_COPY.socket.now : `última señal hace ${segundos} s`;
}

function cambiarSocket(texto, ok) {
  const el = $("estadoSocket");
  if (!el) return;
  el.classList.toggle("ok", ok === true);
  el.classList.toggle("error", ok === false);
  setText("estadoSocketTexto", texto);
  actualizarTiempoSocket();
  window.dispatchEvent(new CustomEvent("mayab:socket", { detail: { texto, ok } }));
}

function iniciarTema() {
  const toggle = $("themeToggle");
  if (!toggle) return;

  const temaGuardado = localStorage.getItem("tema") || "dark";
  document.documentElement.setAttribute("data-theme", temaGuardado);
  document.documentElement.style.colorScheme = temaGuardado;
  actualizarIconosTema(temaGuardado);
  requestAnimationFrame(() => document.documentElement.classList.remove("theme-preload"));

  toggle.addEventListener("click", () => {
    const temaActual = document.documentElement.getAttribute("data-theme");
    const nuevoTema = temaActual === "dark" ? "light" : "dark";
    document.documentElement.setAttribute("data-theme", nuevoTema);
    document.documentElement.style.colorScheme = nuevoTema;
    localStorage.setItem("tema", nuevoTema);
    actualizarIconosTema(nuevoTema);
    tieneCambios = true; // Forzar redibujado de canvases
  });
}

function actualizarIconosTema(tema) {
  const sun = document.querySelector(".icon-sun");
  const moon = document.querySelector(".icon-moon");
  const toggle = $("themeToggle");
  const metaColor = $("themeMetaColor");
  const cambiaA = tema === "dark" ? "claro" : "oscuro";

  if (tema === "dark") {
    if (sun) sun.style.display = "block";
    if (moon) moon.style.display = "none";
    if (metaColor) metaColor.setAttribute("content", "#0c0e14");
  } else {
    if (sun) sun.style.display = "none";
    if (moon) moon.style.display = "block";
    if (metaColor) metaColor.setAttribute("content", "#f4e9e1");
  }

  if (toggle) {
    toggle.setAttribute("aria-label", `Cambiar a modo ${cambiaA}`);
    toggle.setAttribute("title", `Cambiar a modo ${cambiaA}`);
  }
}

function aplicarAnimacionCambio(el, nuevoValor, viejaClave) {
  const viejoValor = metricasPrevias[viejaClave];
  if (nuevoValor === viejoValor) return;

  el.classList.remove("pulse-verde", "pulse-rojo");
  void el.offsetWidth; // trigger reflow

  if (nuevoValor > viejoValor) {
    el.classList.add("pulse-verde");
  } else if (nuevoValor < viejoValor) {
    el.classList.add("pulse-rojo");
  }
  metricasPrevias[viejaClave] = nuevoValor;
}

function renderDineroMetrica(el, valor) {
  if (!el) return;
  let amount = el.querySelector(".metric-amount");
  let unit = el.querySelector(".metric-unit");
  if (!amount || !unit) {
    el.replaceChildren();
    amount = document.createElement("span");
    amount.className = "metric-amount";
    unit = document.createElement("small");
    unit.className = "metric-unit";
    unit.textContent = "USD";
    el.append(amount, unit);
  }
  amount.textContent = dineroMetrica.format(valor);
}

function renderCantidadMetrica(el, valor) {
  if (!el) return;
  const amount = el.querySelector(".metric-amount");
  if (amount) amount.textContent = valor;
  else el.textContent = valor;
}

let ajusteMetricasPendiente = false;

function ajustarTamanioMetrica(el) {
  if (!el || el.clientWidth <= 0) return;

  const minimo = 16;
  const maximo = 84;
  let bajo = minimo;
  let alto = maximo;

  // Búsqueda binaria: usa el mayor tamaño que cabe en una sola línea.
  for (let i = 0; i < 8; i += 1) {
    const candidato = (bajo + alto) / 2;
    el.style.setProperty("--metric-font-size", `${candidato}px`);
    if (el.scrollWidth <= el.clientWidth + 0.5) bajo = candidato;
    else alto = candidato;
  }

  el.style.setProperty("--metric-font-size", `${Math.floor(bajo * 10) / 10}px`);
}

function ajustarMetricasVisibles() {
  ajusteMetricasPendiente = false;
  document.querySelectorAll("[data-fit-metric]").forEach(ajustarTamanioMetrica);
}

function programarAjusteMetricas() {
  if (ajusteMetricasPendiente) return;
  ajusteMetricasPendiente = true;
  requestAnimationFrame(ajustarMetricasVisibles);
}

function iniciarAjusteMetricas() {
  programarAjusteMetricas();
  if (!("ResizeObserver" in window)) {
    window.addEventListener("resize", programarAjusteMetricas, { passive: true });
    return;
  }

  const observer = new ResizeObserver(programarAjusteMetricas);
  document.querySelectorAll(".metricas article").forEach((card) => observer.observe(card));
}

let parActivo = "ALL";

function renderizar(datos) {
  const selector = $("parSelector");
  if (selector && datos.paresActivos) {
    const paresActuales = new Set([...selector.options].map(o => o.value));
    datos.paresActivos.forEach(par => {
      if (!paresActuales.has(par)) {
        const option = document.createElement("option");
        option.value = par;
        option.textContent = par;
        selector.appendChild(option);
      }
    });
    if (!selector.dataset.listener) {
      selector.addEventListener("change", (e) => {
        parActivo = e.target.value;
        if (ultimoEstado) renderizar(ultimoEstado);
      });
      selector.dataset.listener = "true";
    }
  }

  const executionPnlVal = datos.metricas.utilidadAcumuladaUsd;
  const pnlExecutionEl = $("pnlExecution");
  renderCantidadMetrica(pnlExecutionEl, dinero.format(executionPnlVal));
  aplicarAnimacionCambio(pnlExecutionEl, executionPnlVal, "pnlExecution");

  const rebalanceCostVal = datos.metricas.costoRebalanceoAcumuladoUsd || 0;
  const pnlRebalanceEl = $("pnlRebalance");
  renderCantidadMetrica(pnlRebalanceEl, dinero.format(-rebalanceCostVal));
  aplicarAnimacionCambio(pnlRebalanceEl, -rebalanceCostVal, "pnlRebalance");

  const capitalDeltaVal = datos.metricas.capitalActualUsd - datos.metricas.capitalInicialUsd;
  const m2mVal = capitalDeltaVal - executionPnlVal;
  const pnlMarkToMarketEl = $("pnlMarkToMarket");
  renderCantidadMetrica(pnlMarkToMarketEl, dinero.format(m2mVal));
  aplicarAnimacionCambio(pnlMarkToMarketEl, m2mVal, "pnlMarkToMarket");

  const pnlVal = capitalDeltaVal;
  const pnlEl = $("pnl");
  renderCantidadMetrica(pnlEl, dinero.format(pnlVal));
  aplicarAnimacionCambio(pnlEl, pnlVal, "pnl");

  // Métricas secundarias
  const sharpeVal = datos.metricas.sharpeRatio;
  const sharpeEl = $("sharpe");
  renderCantidadMetrica(sharpeEl, formato(sharpeVal, 2));
  aplicarAnimacionCambio(sharpeEl, sharpeVal, "sharpe");

  const winRateVal = datos.metricas.winRate;
  const winRateEl = $("winRate");
  renderCantidadMetrica(winRateEl, formato(winRateVal * 100, 1));
  aplicarAnimacionCambio(winRateEl, winRateVal, "winRate");

  const sortinoVal = datos.metricas.sortinoRatio || 0.0;
  const sortinoEl = $("sortinoRatio");
  if (sortinoEl) {
    renderCantidadMetrica(sortinoEl, formato(sortinoVal, 2));
    aplicarAnimacionCambio(sortinoEl, sortinoVal, "sortinoRatio");
  }

  const kellyVal = datos.metricas.kellyCriterion || 0.0;
  const kellyEl = $("kellyCriterion");
  if (kellyEl) {
    renderCantidadMetrica(kellyEl, formato(kellyVal * 100, 1));
    aplicarAnimacionCambio(kellyEl, kellyVal, "kellyCriterion");
  }

  const tobiVal = datos.metricas.tobi || 0.0;
  const tobiEl = $("tobi");
  if (tobiEl) {
    renderCantidadMetrica(tobiEl, formato(tobiVal, 2));
    aplicarAnimacionCambio(tobiEl, tobiVal, "tobi");
  }

  const bayesianVal = datos.metricas.bayesian || 0.0;
  const bayesianEl = $("bayesian");
  if (bayesianEl) {
    renderCantidadMetrica(bayesianEl, formato(bayesianVal * 100, 1));
    aplicarAnimacionCambio(bayesianEl, bayesianVal, "bayesian");
  }

  const drawdownVal = datos.metricas.maxDrawdownUsd;
  const drawdownEl = $("maxDrawdown");
  renderDineroMetrica(drawdownEl, drawdownVal);
  aplicarAnimacionCambio(drawdownEl, drawdownVal, "maxDrawdown");

  const opsTotalesVal = datos.metricas.operacionesTotales;
  const opsTotalesEl = $("operacionesTotales");
  renderCantidadMetrica(opsTotalesEl, numero.format(opsTotalesVal));
  aplicarAnimacionCambio(opsTotalesEl, opsTotalesVal, "operacionesTotales");

  const opsFallidasVal = datos.metricas.operacionesFallidas || 0;
  const opsFallidasEl = $("operacionesFallidas");
  if (opsFallidasEl) {
    renderCantidadMetrica(opsFallidasEl, numero.format(opsFallidasVal));
    aplicarAnimacionCambio(opsFallidasEl, opsFallidasVal, "operacionesFallidas");
  }

  const rebalanceosVal = datos.metricas.rebalanceosTotales || 0;
  const rebalanceosEl = $("rebalanceosTotales");
  if (rebalanceosEl) {
    renderCantidadMetrica(rebalanceosEl, numero.format(rebalanceosVal));
    aplicarAnimacionCambio(rebalanceosEl, rebalanceosVal, "rebalanceosTotales");
  }

  // Labels generales
  $("riesgo").textContent = datos.metricas.estadoRiesgo;
  $("trabajadores").textContent = `${datos.metricas.trabajadores} trabajadores`;
  actualizarMejorDiferencial(datos);
  programarAjusteMetricas();

  // Banners y Badges
  const cbBanner = $("circuitBreakerBanner");
  if (cbBanner) {
    cbBanner.hidden = !datos.metricas.circuitBreakerActivo;
  }
  const consBadge = $("modoConservadorBadge");
  if (consBadge) {
    consBadge.hidden = !datos.metricas.modoConservador;
  }
  actualizarModoOperacion(datos);
  renderProvenance(datos);

  // Renderizado de paneles secundarios.
  renderMercado(datos);
  renderHeatmapOportunidades(datos);
  dibujarPnlLive(datos);
  renderLatencias(datos);
  renderPipeline(datos.telemetriaPipeline, datos);
  renderJudgeReadiness(datos);
  renderBenchmarkCobertura();
  renderEdgePanel(datos);
  renderBalances(datos);
  renderTransferencias(datos);
  renderConfig(datos);
  renderOportunidades(datos);
  renderDetalleOportunidad(datos);
  renderOperaciones(datos);
  renderEventosEjecucion(datos);
  renderTrazasEjecucion(datos);
  renderRebalanceos(datos);
  renderAuditoriaDecisiones(datos);
  renderGenetico(datos);
  renderMlEdge(datos.mlEdge);
  renderResumenLlm(datos);
  actualizarInputsGaUnaVez(datos.genetico);
  renderExchanges(datos);
  dibujarSeries(datos);
  actualizarInputsConfigUnaVez(datos.configuracion);
  aplicarFiltrosTablas();
}

function coberturaMercado(datos) {
  const exchanges = datos?.exchangesActivos || {};
  const configurados = Object.keys(exchanges).length;
  const activos = Object.values(exchanges).filter(Boolean).length;
  const staleMs = Number(datos?.configuracion?.staleMs || 0);
  const generadoMs = Date.parse(datos?.generadoEn || "");
  const unicos = (quotes) => new Set(quotes.map((cot) => cot.exchange).filter(Boolean)).size;
  const quotes = (datos?.cotizaciones || []).filter((cot) => exchanges[cot.exchange] !== false);
  const esFresco = (cot) => {
    const recibidoMs = Date.parse(cot.recibidaEn || "");
    return cot.conectado === true && staleMs > 0 && Number.isFinite(recibidoMs)
      && Number.isFinite(generadoMs) && Math.max(0, generadoMs - recibidoMs) <= staleMs;
  };
  const wsFrescos = unicos(quotes.filter((cot) => esFresco(cot) && cot.ultimoMensaje !== "rest_fallback"));
  const restFallback = unicos(quotes.filter((cot) => esFresco(cot) && cot.ultimoMensaje === "rest_fallback"));
  const ruteables = unicos(quotes.filter((cot) => esFresco(cot) && Number(cot.bid) > 0 && Number(cot.ask) > Number(cot.bid)));
  return { configurados, activos, quotes: quotes.length, wsFrescos, restFallback, ruteables };
}

function actualizarModoOperacion(datos) {
  const badge = $("modoOperacionBadge");
  if (!badge) return;
  const cobertura = coberturaMercado(datos);
  const usaFallback = cobertura.restFallback > 0;
  const usaWsFresco = cobertura.wsFrescos > 0;
  const ahora = Date.now();
  const eventos = datos.eventosEjecucion || [];
  const demoActivo = eventos.some((e) => {
    const t = Date.parse(e.tiempo || "");
    return String(e.tipo || "").startsWith("demo") && Number.isFinite(t) && ahora - t < 60_000;
  });
  const demoRentablePersistente =
    (datos.metricas?.utilidadAcumuladaUsd || 0) > 0 &&
    eventos.some((e) => String(e.tipo || "") === "demo_rentable");
  badge.className = "modo-operacion-badge";
  if (demoRentablePersistente && usaFallback) {
    badge.textContent = "DEMO RENTABLE + REST";
    badge.classList.add("fallback");
  } else if (demoRentablePersistente) {
    badge.textContent = "DEMO RENTABLE";
    badge.classList.add("demo");
  } else if (demoActivo && usaFallback) {
    badge.textContent = "DEMO + REST";
    badge.classList.add("fallback");
  } else if (demoActivo && usaWsFresco) {
    badge.textContent = "DEMO + LIVE";
    badge.classList.add("demo");
  } else if (demoActivo) {
    badge.textContent = "DEMO · SIN FEEDS";
    badge.classList.add("demo");
  } else if (usaFallback) {
    badge.textContent = "REST FALLBACK";
    badge.classList.add("fallback");
  } else if (usaWsFresco) {
    badge.textContent = "LIVE WS";
  } else if (cobertura.quotes > 0) {
    badge.textContent = "WS STALE";
    badge.classList.add("fallback");
  } else {
    badge.textContent = "SIN FEEDS";
    badge.classList.add("fallback");
  }
  
  const conservadorBadge = $("modoConservadorBadge");
  if (conservadorBadge) {
    if (datos.metricas && datos.metricas.modoConservador) {
      conservadorBadge.hidden = false;
    } else {
      conservadorBadge.hidden = true;
    }
  }
}

function renderProvenance(datos) {
  const {
    configurados, activos, wsFrescos: frescos, restFallback, ruteables,
  } = coberturaMercado(datos);
  const rutas = Math.max(0, ruteables * Math.max(0, ruteables - 1));
  const textoDin = `${configurados} adaptadores · ${activos} habilitados · ${frescos} WS frescos · ${restFallback} REST · ${ruteables} ruteables`;
  
  document.querySelectorAll('[data-ui-copy="landingKicker"]').forEach(el => el.textContent = `${textoDin} · ${rutas} rutas ahora`);
  document.querySelectorAll('[data-ui-copy="proofMarket"]').forEach(el => el.textContent = textoDin);
  setText("juryProofMarket", `${frescos} WS frescos · ${ruteables} venues ruteables`);
  
  setText("provenanceData", `${frescos} WS · ${restFallback} REST`);
  setText("provenanceLatency", `Último estado conocido: ${textoDin}`);
  const corrida = datos.corrida || {};
  const id = String(corrida.id || "session");
  setText("provenanceRun", corrida.modo === "observacion_live" ? "observación live" : id);
  const inicio = corrida.iniciadaEn ? new Date(corrida.iniciadaEn).toLocaleTimeString("es-MX", { hour: "2-digit", minute: "2-digit", second: "2-digit" }) : "—";
  setText("provenanceRunMeta", `${corrida.fuentePnl || "simulación"} · inicio ${inicio} · ejecución ${corrida.ejecucionReal ? "externa" : "simulada"}`);

  const fuentePnl = String(corrida.fuentePnl || "").toLowerCase();
  const esSintetica = String(corrida.modo || "").includes("demo")
    || fuentePnl.includes("demo")
    || (datos.eventosEjecucion || []).some((e) => String(e.tipo || "").startsWith("demo"));
  setText("provenanceResultTag", esSintetica ? "Resultado de escenario sintético" : "Resultado simulado con mercado en vivo");
  const reconciliada = (datos.trazasEjecucion || []).find((trace) =>
    ["RECONCILED", "RECONCILED_LOSS"].includes(String(trace.estado || ""))
      && Math.abs(Number(trace.exposicionBtc || 0)) <= 1e-8,
  );
  setText("juryProofExecution", reconciliada ? `${reconciliada.estado} · 0 BTC residual` : "ejecuta la prueba completa");
  setText("juryProofWallets", reconciliada ? "ledger conciliado · reservas liberadas" : `${numero.format((datos.balances || []).length)} wallets visibles`);
}

function actualizarDetallePnl(datos) {
  const el = $("pnlDetalle");
  if (!el) return;

  const operaciones = datos.metricas.operacionesTotales || 0;
  if (operaciones > 0) {
    const esDemoRentable = (datos.eventosEjecucion || []).some((e) => String(e.tipo || "") === "demo_rentable");
    el.textContent = esDemoRentable
      ? `Resultado del escenario: ${numero.format(operaciones)} operaciones con comisiones, slippage, latencia, fills parciales y rebalanceo incluidos.`
      : `Resultado acumulado de ${numero.format(operaciones)} operaciones simuladas después de costos.`;
    return;
  }

  const oportunidades = oportunidadesVigentes(datos);
  if (oportunidades.length === 0) {
    el.textContent = "No hay una ruta con utilidad neta en este momento. Puedes ejecutar el escenario rentable para revisar el flujo completo.";
    return;
  }

  const ejecutables = oportunidades.filter((o) => o.ejecutable);
  if (ejecutables.length > 0) {
    const mejor = ejecutables.sort((a, b) => b.utilidadUsd - a.utilidadUsd)[0];
    el.textContent = `Hay una ruta ejecutable. Mejor utilidad estimada: ${dinero.format(mejor.utilidadUsd)}.`;
    return;
  }

  const mejor = [...oportunidades].sort((a, b) => b.diferencialNetoBps - a.diferencialNetoBps)[0];
  el.textContent = `Ninguna ruta cumple los filtros. Mejor spread neto actual: ${formato(mejor.diferencialNetoBps, 2)} bps (${mejor.razon}).`;
}

// Cargar inputs del formulario una vez
let configInicializada = false;
function actualizarInputsConfigUnaVez(c) {
  if (configInicializada) return;
  const maxBtc = $("inputMaxBtc");
  const minBps = $("inputMinBps");
  const deslizamiento = $("inputDeslizamiento");
  const cooldown = $("inputCooldown");
  const minUtilidad = $("inputMinUtilidad");
  const staleMs = $("inputStaleMs");
  const latenciaRiesgo = $("inputLatenciaRiesgo");
  const retiroAmortizado = $("inputRetiroAmortizado");
  const usdtPremium = $("inputUsdtPremium");
  const circuitBreaker = $("inputCircuitBreaker");
  const circuitVentana = $("inputCircuitVentana");
  const volatilidad = $("inputVolatilidad");
  const volatilidadVentana = $("inputVolatilidadVentana");
  const probFallo = $("inputProbFallo");
  const probMovimiento = $("inputProbMovimiento");
  const movimientoBps = $("inputMovimientoBps");
  const rebalanceUmbral = $("inputRebalanceUmbral");
  const rebalanceTransfer = $("inputRebalanceTransfer");
  const costoRebalanceo = $("inputCostoRebalanceo");
  const cruceUsdUsdt = $("inputCruceUsdUsdt");
  const simularAdversidad = $("inputSimularAdversidad");

  if (maxBtc) maxBtc.value = c.maxOperacionBtc;
  if (minBps) minBps.value = c.minDiferencialNetoBps;
  if (deslizamiento) deslizamiento.value = c.deslizamientoBps;
  if (cooldown) cooldown.value = c.enfriamientoMs;
  if (minUtilidad) minUtilidad.value = c.minUtilidadUsd;
  if (staleMs) staleMs.value = c.staleMs;
  if (latenciaRiesgo) latenciaRiesgo.value = c.latenciaRiesgoBps;
  if (retiroAmortizado) retiroAmortizado.value = c.retiroAmortizadoBps;
  if (usdtPremium) usdtPremium.value = c.usdtUsdPremiumBps;
  if (circuitBreaker) circuitBreaker.value = c.circuitBreakerPerdidaUsd;
  if (circuitVentana) circuitVentana.value = c.circuitBreakerVentanaMin;
  if (volatilidad) volatilidad.value = c.volatilidadUmbralBps;
  if (volatilidadVentana) volatilidadVentana.value = c.volatilidadVentanaSeg;
  if (probFallo) probFallo.value = c.probFalloOrden;
  if (probMovimiento) probMovimiento.value = c.probMovimientoBrusco;
  if (movimientoBps) movimientoBps.value = c.movimientoBruscoBps;
  if (rebalanceUmbral) rebalanceUmbral.value = c.rebalanceUmbralPct;
  if (rebalanceTransfer) rebalanceTransfer.value = c.rebalanceMaxTransferPct;
  if (costoRebalanceo) costoRebalanceo.value = c.costoRebalanceoUsd;
  if (cruceUsdUsdt) cruceUsdUsdt.checked = Boolean(c.permitirCruceUsdUsdt);
  if (simularAdversidad) simularAdversidad.checked = Boolean(c.simularAdversidad);
  inicializarExchangeCostos(c.exchanges || {});

  const btn = $("btnAplicarConfig");
  if (btn) {
    btn.onclick = async () => {
      await aplicarConfig(construirPayloadConfig(), "Configuración guardada");
    };
  }

  document.querySelectorAll(".preset-btn").forEach((pBtn) => {
    pBtn.addEventListener("click", () => {
      document.querySelectorAll(".preset-btn").forEach((b) => b.classList.remove("activo"));
      pBtn.classList.add("activo");
      aplicarPreset(pBtn.dataset.preset);
      if (btn) btn.click();
    });
  });

  configInicializada = true;
}

function aplicarPreset(preset) {
  const defaults = {
    balanceado: { max: 1.0, minBps: 2.0, des: 1.0, util: 2.0, probF: 0.0, probM: 0.0, adv: false, umbral: 20, cb: 500 },
    agresivo: { max: 5.0, minBps: 1.0, des: 5.0, util: 1.0, probF: 0.02, probM: 0.05, adv: true, umbral: 50, cb: 2000 },
    seguro: { max: 0.2, minBps: 5.0, des: 0.5, util: 5.0, probF: 0.0, probM: 0.0, adv: false, umbral: 10, cb: 100 },
    estres: { max: 5.0, minBps: 0.5, des: 10.0, util: 0.5, probF: 0.3, probM: 0.4, adv: true, umbral: 80, cb: 5000 },
  };
  const cfg = defaults[preset] || defaults.balanceado;
  
  const setV = (id, v) => { const el = $(id); if(el) el.value = v; };
  setV("inputMaxBtc", cfg.max);
  setV("inputMinBps", cfg.minBps);
  setV("inputDeslizamiento", cfg.des);
  setV("inputMinUtilidad", cfg.util);
  setV("inputProbFallo", cfg.probF);
  setV("inputProbMovimiento", cfg.probM);
  setV("inputRebalanceUmbral", cfg.umbral);
  setV("inputCircuitBreaker", cfg.cb);
  const sa = $("inputSimularAdversidad"); if(sa) sa.checked = cfg.adv;
}

function construirPayloadConfig() {
  validarInputsConfig();
  return {
    maxOperacionBtc: leerNumero("inputMaxBtc"),
    minDiferencialNetoBps: leerNumero("inputMinBps"),
    deslizamientoBps: leerNumero("inputDeslizamiento"),
    enfriamientoMs: leerEntero("inputCooldown"),
    minUtilidadUsd: leerNumero("inputMinUtilidad"),
    staleMs: leerEntero("inputStaleMs"),
    latenciaRiesgoBps: leerNumero("inputLatenciaRiesgo"),
    retiroAmortizadoBps: leerNumero("inputRetiroAmortizado"),
    usdtUsdPremiumBps: leerNumero("inputUsdtPremium"),
    circuitBreakerPerdidaUsd: leerNumero("inputCircuitBreaker"),
    circuitBreakerVentanaMin: leerEntero("inputCircuitVentana"),
    volatilidadUmbralBps: leerNumero("inputVolatilidad"),
    volatilidadVentanaSeg: leerEntero("inputVolatilidadVentana"),
    permitirCruceUsdUsdt: leerCheckbox("inputCruceUsdUsdt"),
    simularAdversidad: leerCheckbox("inputSimularAdversidad"),
    probFalloOrden: leerNumero("inputProbFallo"),
    probMovimientoBrusco: leerNumero("inputProbMovimiento"),
    movimientoBruscoBps: leerNumero("inputMovimientoBps"),
    rebalanceUmbralPct: leerNumero("inputRebalanceUmbral"),
    rebalanceMaxTransferPct: leerNumero("inputRebalanceTransfer"),
    costoRebalanceoUsd: leerNumero("inputCostoRebalanceo"),
    exchanges: construirPayloadExchanges(),
  };
}

function leerNumero(id) {
  const valor = Number($(id)?.value);
  return Number.isFinite(valor) ? valor : 0;
}

function leerEntero(id) {
  return Math.trunc(leerNumero(id));
}

function leerCheckbox(id) {
  return Boolean($(id)?.checked);
}

function inicializarExchangeCostos(exchanges) {
  const tbody = $("exchangeCostos");
  if (!tbody || tbody.children.length > 0) return;
  Object.entries(exchanges)
    .sort(([a], [b]) => a.localeCompare(b))
    .forEach(([nombre, cfg]) => {
      const tr = document.createElement("tr");
      tr.dataset.exchange = nombre;
      tr.innerHTML = `
        <td>${nombre}</td>
        <td><input class="exchange-input" data-exchange-field="feeTaker" type="number" step="0.0001" min="0" max="0.02" value="${cfg.feeTaker ?? 0}" aria-label="Fee taker ${nombre}"></td>
        <td><input class="exchange-input" data-exchange-field="retiroBtc" type="number" step="0.00001" min="0" max="0.01" value="${cfg.retiroBtc ?? 0}" aria-label="Retiro BTC ${nombre}"></td>
        <td><input class="exchange-input" data-exchange-field="confiabilidad" type="number" step="0.01" min="0" max="1" value="${cfg.confiabilidad ?? 1}" aria-label="Confiabilidad ${nombre}"></td>
      `;
      tbody.appendChild(tr);
    });
}

function construirPayloadExchanges() {
  const tbody = $("exchangeCostos");
  if (!tbody) return {};
  const exchanges = {};
  tbody.querySelectorAll("tr[data-exchange]").forEach((tr) => {
    const nombre = tr.dataset.exchange;
    const item = { nombre };
    tr.querySelectorAll("[data-exchange-field]").forEach((input) => {
      const field = input.dataset.exchangeField;
      item[field] = Number(input.value);
    });
    exchanges[nombre] = item;
  });
  return exchanges;
}

const limitesConfig = {
  inputMaxBtc: [0.01, 10],
  inputMinBps: [0, 100],
  inputDeslizamiento: [0, 50],
  inputCooldown: [0, 10000],
  inputMinUtilidad: [0, 1000],
  inputStaleMs: [100, 30000],
  inputLatenciaRiesgo: [0, 20],
  inputRetiroAmortizado: [0, 50],
  inputUsdtPremium: [0, 100],
  inputCircuitBreaker: [0, 100000],
  inputCircuitVentana: [1, 240],
  inputVolatilidad: [0, 1000],
  inputVolatilidadVentana: [1, 3600],
  inputProbFallo: [0, 1],
  inputProbMovimiento: [0, 1],
  inputMovimientoBps: [0, 100],
  inputRebalanceUmbral: [0, 100],
  inputRebalanceTransfer: [0, 100],
  inputCostoRebalanceo: [0, 1000],
};

function validarInputsConfig() {
  let ok = true;
  Object.entries(limitesConfig).forEach(([id, [min, max]]) => {
    const input = $(id);
    if (!input) return;
    const value = Number(input.value);
    const valido = Number.isFinite(value) && value >= min && value <= max;
    input.classList.toggle("input-error", !valido);
    input.title = valido ? "" : `Valor permitido: ${min} a ${max}`;
    ok = ok && valido;
  });
  document.querySelectorAll(".exchange-input").forEach((input) => {
    const min = Number(input.min || 0);
    const max = Number(input.max || 1);
    const value = Number(input.value);
    const valido = Number.isFinite(value) && value >= min && value <= max;
    input.classList.toggle("input-error", !valido);
    input.title = valido ? "" : `Valor permitido: ${min} a ${max}`;
    ok = ok && valido;
  });
  const feedback = $("configFeedback");
  if (!ok) mostrarFeedback(feedback, "Revisa los campos marcados antes de aplicar.", false);
  return ok;
}

async function aplicarConfig(payload, mensajeOk) {
  const feedback = $("configFeedback");
  if (!validarInputsConfig()) return;
  try {
    const res = await fetch("/api/config", {
      method: "POST",
      headers: headersMutacion({ "Content-Type": "application/json" }),
      body: JSON.stringify(payload),
    });
    if (feedback) {
      if (res.ok) {
        mostrarFeedback(feedback, mensajeOk, true);
        setTimeout(() => {
          feedback.textContent = "";
        }, 3000);
      } else {
        mostrarFeedback(feedback, `No se pudo guardar: ${await mensajeErrorApi(res, "error de API")}`, false);
      }
    }
  } catch (err) {
    mostrarFeedback(feedback, "Error de red al guardar configuración", false);
  }
}

const presets = {
  balanceado: {
    inputMaxBtc: 0.18,
    inputMinBps: 0.65,
    inputDeslizamiento: 0.35,
    inputCooldown: 1400,
    inputMinUtilidad: 1.25,
    inputStaleMs: 4500,
    inputLatenciaRiesgo: 0.08,
    inputRetiroAmortizado: 0.12,
    inputUsdtPremium: 1.5,
    inputCircuitBreaker: 500,
    inputCircuitVentana: 15,
    inputVolatilidad: 50,
    inputVolatilidadVentana: 60,
    inputProbFallo: 0.015,
    inputProbMovimiento: 0.02,
    inputMovimientoBps: 7,
    inputRebalanceUmbral: 35,
    inputRebalanceTransfer: 35,
    inputCostoRebalanceo: 5,
    inputCruceUsdUsdt: false,
    inputSimularAdversidad: true,
  },
  agresivo: {
    inputMaxBtc: 0.35,
    inputMinBps: 0.25,
    inputDeslizamiento: 0.22,
    inputCooldown: 450,
    inputMinUtilidad: 0.5,
    inputStaleMs: 7000,
    inputLatenciaRiesgo: 0.04,
    inputRetiroAmortizado: 0.08,
    inputUsdtPremium: 1,
    inputCircuitBreaker: 900,
    inputCircuitVentana: 20,
    inputVolatilidad: 90,
    inputVolatilidadVentana: 45,
    inputProbFallo: 0.01,
    inputProbMovimiento: 0.015,
    inputMovimientoBps: 5,
    inputRebalanceUmbral: 45,
    inputRebalanceTransfer: 50,
    inputCostoRebalanceo: 4,
    inputCruceUsdUsdt: false,
    inputSimularAdversidad: true,
  },
  seguro: {
    inputMaxBtc: 0.08,
    inputMinBps: 1.6,
    inputDeslizamiento: 0.8,
    inputCooldown: 2500,
    inputMinUtilidad: 4,
    inputStaleMs: 2200,
    inputLatenciaRiesgo: 0.22,
    inputRetiroAmortizado: 0.18,
    inputUsdtPremium: 2.5,
    inputCircuitBreaker: 220,
    inputCircuitVentana: 10,
    inputVolatilidad: 28,
    inputVolatilidadVentana: 90,
    inputProbFallo: 0.015,
    inputProbMovimiento: 0.02,
    inputMovimientoBps: 7,
    inputRebalanceUmbral: 25,
    inputRebalanceTransfer: 25,
    inputCostoRebalanceo: 8,
    inputCruceUsdUsdt: false,
    inputSimularAdversidad: true,
  },
  estres: {
    inputMaxBtc: 0.18,
    inputMinBps: 0.9,
    inputDeslizamiento: 1.2,
    inputCooldown: 1100,
    inputMinUtilidad: 1.5,
    inputStaleMs: 2600,
    inputLatenciaRiesgo: 0.35,
    inputRetiroAmortizado: 0.35,
    inputUsdtPremium: 4,
    inputCircuitBreaker: 180,
    inputCircuitVentana: 8,
    inputVolatilidad: 22,
    inputVolatilidadVentana: 30,
    inputProbFallo: 0.14,
    inputProbMovimiento: 0.18,
    inputMovimientoBps: 18,
    inputRebalanceUmbral: 20,
    inputRebalanceTransfer: 25,
    inputCostoRebalanceo: 12,
    inputCruceUsdUsdt: false,
    inputSimularAdversidad: true,
  },
};

function iniciarPresets() {
  document.querySelectorAll("[data-preset]").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const nombre = btn.dataset.preset;
      const preset = presets[nombre];
      if (!preset) return;
      Object.entries(preset).forEach(([id, valor]) => {
        const input = $(id);
        if (!input) return;
        if (input.type === "checkbox") input.checked = Boolean(valor);
        else input.value = valor;
      });
      document.querySelectorAll("[data-preset]").forEach((otro) => otro.classList.toggle("activo", otro === btn));
      await aplicarConfig(construirPayloadConfig(), `Preset ${btn.textContent.trim()} aplicado`);
    });
  });
}

function iniciarDemo() {
  const btnFinal = $("btnDemoFinal");
  const btnFinalTop = $("btnDemoFinalTop");
  const btnFinalHero = $("btnJuryProofHero");
  const btnReset = $("btnResetDemo");
  const btnCaos = $("btnDemoCaos");
  if (btnFinal) {
    btnFinal.addEventListener("click", prepararDemoFinal);
  }
  if (btnFinalTop) {
    btnFinalTop.addEventListener("click", prepararDemoFinal);
  }
  if (btnFinalHero) {
    btnFinalHero.addEventListener("click", prepararDemoFinal);
  }
  if (btnReset) {
    btnReset.addEventListener("click", reiniciarDemoJurado);
  }
  if (btnCaos) {
    btnCaos.addEventListener("click", ejecutarPruebaCaos);
  }
  document.querySelectorAll("[data-demo]").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const escenario = btn.dataset.demo;
      const feedback = $("demoFeedback");
      const estado = $("demoEstado");
      btn.disabled = true;
      const textoOriginal = btn.textContent;
      btn.textContent = "Ejecutando...";
      try {
        const res = await fetch("/api/demo", {
          method: "POST",
          headers: headersMutacion({ "Content-Type": "application/json" }),
          body: JSON.stringify({ escenario }),
        });
        const ok = res.ok;
        const body = ok ? await res.json() : null;
        if (estado) estado.textContent = ok ? "evento enviado" : "rechazado";
        const detalle = body?.ejecucionBloqueada
          ? "Escenario bloqueado por el circuit breaker. Reinicia la corrida antes de continuar."
          : body?.partialFill
          ? `Fill parcial probado: solicitado ${btc.format(body.requestedQtyBtc || 0)} BTC, ejecutado ${btc.format(body.filledQtyBtc || 0)} BTC.`
          : body?.operacionesInsertadas
          ? `Escenario completado: ${body.operacionesInsertadas} operaciones; GA en generación ${body.generacionGa}.`
          : "Escenario aplicado. Revisa eventos, auditoría y métricas para ver la reacción del motor.";
        const error = ok ? "" : await mensajeErrorApi(res, "No se pudo aplicar escenario");
        mostrarFeedback(feedback, ok ? detalle : `No se pudo aplicar el escenario: ${error}`, ok);
      } catch (e) {
        if (estado) estado.textContent = "error";
        mostrarFeedback(feedback, "Error de red al aplicar el escenario", false);
      } finally {
        btn.disabled = false;
        btn.textContent = textoOriginal;
      }
    });
  });
}

async function ejecutarPruebaCaos() {
  const btn = $("btnDemoCaos");
  const feedback = $("demoFeedback");
  const estado = $("demoEstado");
  const textoOriginal = btn?.textContent || "Ejecutar prueba de estrés";
  if (btn) {
    btn.disabled = true;
    btn.textContent = "Ejecutando prueba…";
  }
  if (estado) estado.textContent = "prueba en curso";
  mostrarFeedback(feedback, "Ejecutando fill parcial, baja liquidez, fallo de segunda pierna, circuit breaker, rebalanceo y recuperación…", true);
  try {
    const res = await fetch("/api/demo/caos", {
      method: "POST",
      headers: headersMutacion({ "Content-Type": "application/json" }),
    });
    if (!res.ok) {
      mostrarFeedback(feedback, `No se pudo completar la prueba: ${await mensajeErrorApi(res, "error de API")}`, false);
      if (estado) estado.textContent = "prueba fallida";
      return;
    }
    const body = await res.json();
    const final = body.estadoFinal || {};
    if (estado) estado.textContent = body.ok ? "resiliencia verificada" : "revisar checks";
    mostrarFeedback(
      feedback,
      `${body.aprobados || 0}/${body.totalChecks || 0} checks superados · exposición residual ${btc.format(final.exposicionResidualBtc || 0)} BTC · circuit breaker ${final.circuitBreakerActivo ? "activo" : "restaurado"} · PnL ${dinero.format(final.pnlFinalUsd || 0)}.`,
      Boolean(body.ok),
    );
    preflightCache = null;
  } catch (_) {
    if (estado) estado.textContent = "error de red";
    mostrarFeedback(feedback, "Error de red durante la prueba de estrés", false);
  } finally {
    if (btn) {
      btn.disabled = false;
      btn.textContent = textoOriginal;
    }
  }
}

async function reiniciarDemoJurado() {
  const btn = $("btnResetDemo");
  const feedback = $("demoFeedback");
  if (btn) btn.disabled = true;
  mostrarFeedback(feedback, "Restableciendo saldos, PnL, riesgo y registro de la prueba…", true);
  try {
    const res = await fetch("/api/demo/reset", {
      method: "POST",
      headers: headersMutacion(),
    });
    if (!res.ok) {
      mostrarFeedback(feedback, `No se pudo reiniciar: ${await mensajeErrorApi(res, "error de API")}`, false);
      return;
    }
    const body = await res.json();
    preflightCache = null;
    mostrarFeedback(feedback, `Prueba ${body.corridaId || "actual"} reiniciada. Ya puedes ejecutar el recorrido completo.`, true);
  } catch (_) {
    mostrarFeedback(feedback, "Error de red al reiniciar la corrida", false);
  } finally {
    if (btn) btn.disabled = false;
  }
}

let demoFinalEnCurso = null;
function prepararDemoFinal() {
  if (!demoFinalEnCurso) {
    demoFinalEnCurso = ejecutarDemoFinal().finally(() => {
      demoFinalEnCurso = null;
    });
  }
  return demoFinalEnCurso;
}

async function ejecutarDemoFinal() {
  const btn = $("btnDemoFinal");
  const btnTop = $("btnDemoFinalTop");
  const btnHero = $("btnJuryProofHero");
  const minuteStatus = $("juryMinuteStatus");
  const feedback = $("demoFeedback");
  const estado = $("demoEstado");
  const textoOriginal = btn?.textContent || "Ejecutar prueba completa";
  const textoTopOriginal = btnTop?.textContent || "Ejecutar prueba completa";
  const textoHeroOriginal = btnHero?.textContent || "Ver una prueba completa";
  minuteStatus?.classList.remove("ok");
  if (btn) {
    btn.disabled = true;
    btn.textContent = "Cargando...";
  }
  if (btnTop) {
    btnTop.disabled = true;
    btnTop.textContent = "Preparando…";
  }
  if (btnHero) {
    btnHero.disabled = true;
    btnHero.textContent = "Ejecutando 6 pruebas…";
  }
  if (estado) estado.textContent = "prueba en curso";
  if (minuteStatus) minuteStatus.textContent = "Ejecutando replay, operación rentable, fallo de segunda pierna, recuperación, saldos, GA y exportación…";
  mostrarFeedback(feedback, "Ejecutando escenario rentable, GA, fill parcial y rebalanceo...", true);

  try {
    const res = await fetch("/api/demo/final", {
      method: "POST",
      headers: headersMutacion({ "Content-Type": "application/json" }),
    });
    if (!res.ok) {
      mostrarFeedback(feedback, `No se pudo ejecutar la prueba: ${await mensajeErrorApi(res, "error de API")}`, false);
      if (estado) estado.textContent = "rechazado";
      if (minuteStatus) minuteStatus.textContent = "La instancia rechazó la prueba; abre el checklist para ver el bloqueo exacto.";
      return false;
    }
    const body = await res.json();
    const checks = body?.preflight?.judgeReadiness || {};
    const lista = body?.ok === true
      && checks.status === "ready"
      && checks.passed === 12
      && checks.total === 12;
    if (estado) estado.textContent = lista ? "evidencia lista" : "evidencia bloqueada";
    const pnl = body?.metricas?.utilidadAcumuladaUsd ?? 0;
    const gen = body?.metricas ? body?.ga?.generacion : null;
    mostrarFeedback(
      feedback,
      lista
        ? `Prueba ${body?.corridaId || "actual"} completa: PnL ${dinero.format(pnl)}, segunda pierna ${body?.riesgoSegundaPierna?.estadoFinal || "conciliada"}, score evolutivo ${body?.mlEdge?.version || "ok"}, GA ${gen ?? body?.mercadoRentable?.generacionGa ?? "activo"}.`
        : `Prueba ${body?.corridaId || "actual"} incompleta: ${checks.passed || 0}/${checks.total || 12} validaciones. Revisa preflight para ver el detalle.`,
      lista,
    );
    if (minuteStatus) {
      minuteStatus.textContent = lista
        ? `12/12 validaciones correctas · prueba ${body?.corridaId || "actual"} · reporte SHA-256 listo.`
        : `${checks.passed || 0}/${checks.total || 12} validaciones correctas · abre preflight para revisar las pendientes.`;
      minuteStatus.classList.toggle("ok", lista);
    }
    preflightCache = body?.preflight || null;
    ultimoPreflightMs = preflightCache ? Date.now() : 0;
    if ($("tab-logs")?.classList.contains("activo")) {
      fetch("/api/lab/sweep")
        .then(async (lab) => {
          if (lab.ok) renderLabSweep(await lab.json());
        })
        .catch(() => {});
    }
    const evidenceTab = $("tab-evidence");
    if (evidenceTab?.classList.contains("activo") && evidenceTab.dataset.deferEvidence !== "true") {
      cargarEvidenceLab();
    }
    return lista;
  } catch (e) {
    if (estado) estado.textContent = "error";
    mostrarFeedback(feedback, "Error de red al ejecutar la prueba", false);
    if (minuteStatus) {
      minuteStatus.classList.remove("ok");
      minuteStatus.textContent = "La prueba no terminó; revisa la conexión y vuelve a intentarlo.";
    }
    return false;
  } finally {
    if (btn) {
      btn.disabled = false;
      btn.textContent = textoOriginal;
    }
    if (btnTop) {
      btnTop.disabled = false;
      btnTop.textContent = textoTopOriginal;
    }
    if (btnHero) {
      btnHero.disabled = false;
      btnHero.textContent = textoHeroOriginal;
    }
  }
}

let gaInicializada = false;
async function cargarConfigGa() {
  try {
    const res = await fetch("/api/ga/config");
    if (!res.ok) return;
    const cfg = await res.json();
    actualizarInputsGaUnaVez({
      poblacion: cfg.tamanoPoblacion,
      tasaMutacion: cfg.tasaMutacion,
      tasaCruce: cfg.tasaCruce,
    });
  } catch (e) {
    debugWarn("No se pudo cargar config GA", e);
  }
}

async function actualizarInputsGaUnaVez(g) {
  if (gaInicializada || !g) return;
  const poblacion = $("inputGaPoblacion");
  const mutacion = $("inputGaMutacion");
  const cruce = $("inputGaCruce");
  if (poblacion) poblacion.value = g.poblacion ?? g.tamanoPoblacion ?? 50;
  if (mutacion) mutacion.value = g.tasaMutacion ?? 0.15;
  if (cruce) cruce.value = g.tasaCruce ?? 0.72;

  const feedback = $("gaFeedback");
  const btnAplicar = $("btnAplicarGa");
  if (btnAplicar) {
    btnAplicar.onclick = async () => {
      btnAplicar.disabled = true;
      btnAplicar.textContent = "Aplicando...";
      try {
        const res = await fetch("/api/ga/config", {
          method: "POST",
          headers: headersMutacion({ "Content-Type": "application/json" }),
          body: JSON.stringify({
            tamanoPoblacion: parseInt(poblacion.value),
            tasaMutacion: parseFloat(mutacion.value),
            tasaCruce: parseFloat(cruce.value),
          }),
        });
        const mensaje = res.ok ? "GA actualizado: nueva población lista para competir" : await mensajeErrorApi(res, "No se pudo actualizar GA");
        mostrarFeedback(feedback, res.ok ? mensaje : `No se pudo actualizar GA: ${mensaje}`, res.ok);
      } catch (e) {
        mostrarFeedback(feedback, "Error de red al actualizar GA", false);
      } finally {
        btnAplicar.disabled = false;
        btnAplicar.textContent = "Aplicar GA";
      }
    };
  }

  const btnEvolucionar = $("btnEvolucionarGa");
  if (btnEvolucionar) {
    btnEvolucionar.onclick = evolucionarGa;
  }
  gaInicializada = true;
}

function headersMutacion(extra = {}) {
  const headers = { ...extra };
  const token = localStorage.getItem("mayabAdminToken");
  if (token) headers.Authorization = `Bearer ${token}`;
  return headers;
}

async function evolucionarGa() {
  if (gaAutoEnCurso) return;

  const feedback = $("gaFeedback");
  const btnEvolucionar = $("btnEvolucionarGa");
  const genAntes = ultimoEstado?.genetico?.generacion ?? null;
  gaAutoEnCurso = true;

  if (btnEvolucionar) {
    btnEvolucionar.disabled = true;
    btnEvolucionar.textContent = "Compitiendo...";
    mostrarFeedback(feedback, "El GA está probando estrategias contra el replay...", true);
  }

  try {
    const res = await fetch("/api/ga/evolucionar", {
      method: "POST",
      headers: headersMutacion({ "Content-Type": "application/json" }),
      body: JSON.stringify({ usarReplaySiVacio: true, muestras: 96 }),
    });
    if (!res.ok) {
      mostrarFeedback(feedback, `No se pudo evolucionar GA: ${await mensajeErrorApi(res, "error de API")}`, false);
      return;
    }

    const resultado = await res.json();
    const genetico =
      resultado.ga ||
      (await fetch("/api/ga/estado").then((estado) =>
        estado.ok ? estado.json() : null,
      ));
    if (!genetico) return;
    renderGenetico({ genetico });
    if (ultimoEstado) ultimoEstado.genetico = genetico;

    const genDespues = genetico.generacion;
    const muestras = genetico.operacionesEvaluadas || 0;
    const fuente = resultado.fuente === "replay_sintetico" ? "replay sintético" : "historial real";
    const prefijo = genAntes === null ? `Gen ${genDespues}` : `Gen ${genAntes} -> ${genDespues}`;
    const detalle = muestras > 0
      ? `${muestras} operaciones evaluadas (${fuente}); delta y campeón retenido actualizados`
      : "sin operaciones para aprender; población sigue explorando";
    mostrarFeedback(feedback, `GA ${prefijo}: ${detalle}`, true);
    marcarCambio($("gaGeneracion"));
    marcarCambio($("gaPesos"));
  } catch (e) {
    mostrarFeedback(feedback, "Error de red al evolucionar GA", false);
  } finally {
    gaAutoEnCurso = false;
    if (btnEvolucionar) {
      btnEvolucionar.disabled = false;
      btnEvolucionar.textContent = "Evolucionar ahora";
    }
  }
}

/**
 * @description Actualiza el panel de mercado, mostrando las cotizaciones (bid/ask), spreads
 * y la fuente de datos (WS/REST) para cada exchange activo.
 * @param {Object} datos - El estado público actual.
 */
function renderMercado(datos) {
  const container = $("exchangeLista");
  if (!container) return;

  const diferenciales = datos.cotizaciones.map((c) => c.ask - c.bid).filter((s) => s > 0);
  const maxDiferencial = Math.max(...diferenciales, 1);

  datos.cotizaciones.forEach((c, index) => {
    let art = container.children[index];
    if (!art) {
      art = document.createElement("article");
      art.className = "exchange";
      art.innerHTML = `
        <header>
          <h3></h3>
          <span class="latencia-chip"></span>
        </header>
        <div class="precios">
          <div class="precio bid"><span>Compra</span><strong></strong></div>
          <div class="precio ask"><span>Venta</span><strong></strong></div>
        </div>
        <div class="barra-diferencial" aria-label="Diferencial"><div class="fill"></div></div>
        <div class="exchange-meta">
          <span class="edad"></span>
          <span class="fuente"></span>
        </div>
      `;
      container.appendChild(art);
    }

    const recibida = Date.parse(c.recibidaEn || "");
    const generado = Date.parse(datos.generadoEn || "");
    const edadMs = Number.isFinite(recibida) && Number.isFinite(generado) ? Math.max(0, generado - recibida) : 0;
    const fuente = c.ultimoMensaje === "rest_fallback" ? "REST fallback" : "WebSocket";
    const latenciaMs = Number(c.latenciaMs || 0);
    const diferencial = Math.max(c.ask - c.bid, 0);

    const h3 = art.querySelector("header h3");
    if (h3.textContent !== c.exchange) h3.textContent = c.exchange;

    const latChip = art.querySelector("header .latencia-chip");
    const nuevaClase = `latencia-chip ${latenciaMs > 850 ? "alta" : latenciaMs > 420 ? "media" : "buena"}`;
    if (latChip.className !== nuevaClase) latChip.className = nuevaClase;
    latChip.textContent = c.timestampConfiable ? `${c.latenciaMs || 0} ms` : "wire n/d";

    art.querySelector(".precio.bid strong").textContent = dinero.format(c.bid);
    art.querySelector(".precio.ask strong").textContent = dinero.format(c.ask);

    art.querySelector(".barra-diferencial div.fill").style.width = `${Math.min(100, (diferencial / maxDiferencial) * 100)}%`;

    art.querySelector(".exchange-meta .edad").innerHTML = `Edad del libro <strong>${numero.format(edadMs)} ms</strong>`;
    const integridad = String(c.integrityStatus || "sin validar").replaceAll("_", " ");
    const secuencia = c.exchangeSequence == null ? "seq n/d" : `seq ${numero.format(c.exchangeSequence)}`;
    const integridadMetricas = `gaps ${numero.format(c.sequenceGaps || 0)} · crc fail ${numero.format(c.checksumFailures || 0)} · inválido ${numero.format(c.invalidatedMs || 0)} ms`;
    art.querySelector(".exchange-meta .fuente").textContent = `${fuente} · ${integridad} · ${secuencia} · ${integridadMetricas}`;
  });

  while (container.children.length > datos.cotizaciones.length) {
    container.removeChild(container.lastChild);
  }
}

async function renderJudgeReadiness(datos) {
  const container = $("judgeReadiness");
  if (!container) return;
  const ahora = Date.now();
  if (!preflightCache || ahora - ultimoPreflightMs > 10_000) {
    if (!preflightEnCurso) {
      preflightEnCurso = true;
      fetch("/api/preflight")
        .then((res) => (res.ok ? res.json() : null))
        .then((json) => {
          if (json) {
            preflightCache = json;
            ultimoPreflightMs = Date.now();
          }
        })
        .catch(() => {})
        .finally(() => {
          preflightEnCurso = false;
        });
    }
  }

  const readiness = preflightCache?.judgeReadiness;
  if (!readiness) {
    const ops = datos?.metricas?.operaciones || 0;
    const auditorias = (datos?.auditoriaDecisiones || []).length;
    container.innerHTML = `
      <div class="judge-score">
        <strong>...</strong>
        <span>calculando</span>
      </div>
      <p>Calculando validaciones. Estado actual: ${numero.format(ops)} operaciones y ${numero.format(auditorias)} decisiones registradas.</p>
    `;
    return;
  }

  const checks = readiness.checks || [];
  const byName = Object.fromEntries(checks.map((c) => [c.name, c.ok === true]));
  const faltantes = checks.filter((c) => !c.ok).map((c) => etiquetaCheck(c.name));
  const venues = preflightCache?.venues || {};
  const evidencia = preflightCache?.evidenceMatrix || [];
  const cb = datos?.metricas?.circuitBreakerActivo ? "ACTIVO" : "inactivo";
  const modCon = datos?.metricas?.modoConservador ? "ON" : "OFF";
  const cbUsd = dinero.format(datos?.configuracion?.circuitBreakerPerdidaUsd || 0);
  const card = (titulo, ok, texto) => `
    <div class="judge-card ${ok ? "ok" : "bad"}">
      <strong>${escapeHtml(titulo)}</strong>
      <p>${escapeHtml(texto)}</p>
      <span class="${ok ? "chip-ok" : "chip-bad"}">${ok ? "Ok" : "Revisar"}</span>
    </div>
  `;

  container.innerHTML = `
    <div class="judge-score">
      <strong>${readiness.status === "ready" ? "READY" : "BLOCKED"}</strong>
      <span>capacidad operativa · ${numero.format(venues.conWebSocketFresco || 0)} live / mínimo ${numero.format(venues.minimosRequeridos || 2)}</span>
    </div>
    <div class="judge-cards">
      ${card("Control de estrategia", byName.netProfitCalculation && byName.feesSlippageLatency && byName.exports, "Umbrales, costos, riesgo, exchanges, GA y exports se pueden revisar o mover desde UI/API.")}
      ${card("Robustez bajo estrés", byName.riskGuards && byName.safeDemoMode, `Circuit breaker: ${cb} (${cbUsd}). Modo conservador: ${modCon}. Los resultados aparecen en la auditoría.`)}
      ${card("Inventario operativo", byName.walletAccounting && byName.partialFillSupport, "Balances por exchange, fill parcial y rebalanceo con costo explícito.")}
      ${card("Decisión explicable", byName.decisionInspector && byName.mlEdgeExplainable, "Cada ruta muestra score, pesos GA, EV, costos y razón de aceptación o descarte.")}
      ${card("Mercado conectado", byName.realTimeOrderBooks, "Libros de órdenes actualizados, latencia por exchange y fallback REST visible.")}
    </div>
    <p><strong>Venues:</strong> ${numero.format(venues.configurados || 0)} adaptadores configurados · ${numero.format(venues.habilitados || 0)} habilitados · ${numero.format(venues.conWebSocketFresco || 0)} LIVE · ${numero.format(venues.conLibroRuteable || 0)} ruteables.</p>
    <p><strong>Evidencia de corrida:</strong> ${evidencia.map((e) => `${escapeHtml(e.claim)}=${escapeHtml(e.status)}`).join(" · ") || "esperando preflight"}. Un WARN significa “aún no ejecutado”, no falla operativa.</p>
    <p>${faltantes.length ? `Evidencia opcional pendiente: ${escapeHtml(faltantes.join(", "))}` : "Evidencia runtime completa y verificable."}</p>
  `;
}

function renderBenchmarkCobertura() {
  const container = $("benchmarkCobertura");
  if (!container) return;
  const cobertura = preflightCache?.judgeReadiness?.coberturaFinalista;
  if (!cobertura) {
    container.innerHTML = `
      <div class="benchmark-summary">
        <strong>...</strong>
        <span>calculando cobertura</span>
      </div>
    `;
    return;
  }

  const dims = cobertura.dimensiones || [];
  const status = cobertura.status === "completo" ? "completo" : "accionable";
  container.innerHTML = `
    <div class="benchmark-summary ${status}">
      <strong>${numero.format(cobertura.cubiertas || 0)}/${numero.format(cobertura.total || dims.length || 0)}</strong>
      <span>${escapeHtml(status)}</span>
      <small>${escapeHtml(cobertura.lectura || "")}</small>
    </div>
    <div class="benchmark-items">
      ${dims
        .slice(0, 8)
        .map(
          (item) => `
            <article class="${item.ok ? "ok" : "bad"}">
              <span>${item.ok ? "Ok" : "Revisar"}</span>
              <strong>${escapeHtml(etiquetaBenchmark(item.nombre))}</strong>
              <p>${escapeHtml(item.evidencia || "")}</p>
              <small>${escapeHtml(item.dondeVerificar || "")}</small>
            </article>
          `,
        )
        .join("")}
    </div>
  `;
}

function estadoReadiness(status) {
  const labels = {
    ready: "listo",
    review: "revisar",
  };
  return labels[status] || status || "revisar";
}

function etiquetaCheck(nombre) {
  const labels = {
    realTimeOrderBooks: "libros de órdenes en vivo",
    netProfitCalculation: "utilidad neta",
    feesSlippageLatency: "costos/latencia",
    partialFillSupport: "fill parcial",
    walletAccounting: "carteras",
    decisionInspector: "inspector de decisiones",
    mlEdgeExplainable: "score evolutivo",
    riskGuards: "guardas de riesgo",
    safeDemoMode: "modo de simulación",
    exports: "exports",
  };
  return labels[nombre] || nombre || "validación";
}

function etiquetaBenchmark(nombre) {
  const labels = {
    parametrizacion_profunda: "Parametrización",
    robustez_adversa: "Robustez adversa",
    wallets_rebalanceo: "Wallets y rebalanceo",
    ui_visualizacion_jurado: "UI de jurado",
    metricas_latency_replay: "Métricas y replay",
    documentacion_tests_deploy: "Docs, tests y deploy",
    auditoria_durable_exports: "Auditoría y exports",
    auditoria_local_exports: "Auditoría y exports",
    ia_explicable_ga: "IA explicable + GA",
  };
  return labels[nombre] || nombre || "Cobertura";
}

function renderEdgePanel(datos) {
  const edge = datos?.mlEdge;
  const features = $("edgeFeatures");
  if (!edge) {
    setText("edgeEstado", "esperando auditoría");
    setText("edgeExplicacion", "Aún no hay una decisión para analizar. Ejecuta un escenario o espera una oportunidad para calcular el score.");
    setText("edgeEv", dinero.format(0));
    setText("edgeConfianza", "0.0%");
    setText("edgeSurvival", "0.0%");
    setText("edgeFill", "0.0%");
    setText("edgeAdverse", "0.00 bps");
    actualizarEdgePlano(null);
    if (features) features.textContent = "";
    return;
  }

  const estado = $("edgeEstado");
  if (estado) {
    estado.textContent = `${edge.modelo || "Mayab Edge"} · ${edge.decision || "sin decisión"}`;
    estado.classList.toggle("ok", edge.activo === true);
  }
  setText("edgeExplicacion", edge.explicacion || "Score explicable calculado desde la última decisión auditada.");
  setText("edgeEv", dinero.format(edge.expectedValueUsd || 0));
  setText("edgeConfianza", `${formato((edge.confianza || 0) * 100, 1)}%`);
  setText("edgeSurvival", `${formato((edge.survivalProbability || 0) * 100, 1)}%`);
  setText("edgeFill", `${formato((edge.fillProbability || 0) * 100, 1)}%`);
  setText("edgeAdverse", `${formato(edge.adverseSelectionBps || 0, 2)} bps`);
  actualizarEdgePlano(edge);

  if (!features) return;
  features.textContent = "";
  (edge.features || []).forEach((f) => {
    const row = document.createElement("div");
    row.className = "edge-feature";
    const pct = Math.max(0, Math.min(100, (f.contribucion || 0) * 100));
    row.innerHTML = `
      <span>${escapeHtml(nombreFeature(f.nombre))}</span>
      <div class="edge-track"><div style="width:${pct}%"></div></div>
      <strong>${formato((f.valor || 0) * 100, 0)}% × ${formato((f.peso || 0) * 100, 0)}%</strong>
    `;
    features.appendChild(row);
  });
}

function actualizarEdgePlano(edge) {
  const plot = { left: 54, top: 24, width: 266, height: 180 };
  const conf = limitar01(edge?.confianza || 0);
  const survival = limitar01(edge?.survivalProbability || 0);
  const fill = limitar01(edge?.fillProbability || 0);
  const ev = Number(edge?.expectedValueUsd || 0);
  const adverse = Math.max(0, Number(edge?.adverseSelectionBps || 0));
  const evNorm = clamp(ev / 25, -1, 1);
  const adverseNorm = -clamp(adverse / 20, 0, 1);
  const x = (valor) => plot.left + limitar01(valor) * plot.width;
  const y = (valor) => plot.top + (1 - (clamp(valor, -1, 1) + 1) / 2) * plot.height;
  const puntos = [
    ["edgePointEv", "edgeLabelEv", x(conf), y(evNorm), `EV ${dinero.format(ev)}`],
    ["edgePointConfianza", "edgeLabelConfianza", x(conf), y(0.18), `Conf ${formato(conf * 100, 0)}%`],
    ["edgePointSurvival", "edgeLabelSurvival", x(survival), y(0.52), `Sup ${formato(survival * 100, 0)}%`],
    ["edgePointFill", "edgeLabelFill", x(fill), y(0.76), `Fill ${formato(fill * 100, 0)}%`],
    ["edgePointAdverse", "edgeLabelAdverse", x(1 - clamp(adverse / 20, 0, 1)), y(adverseNorm), `Adv ${formato(adverse, 1)}bps`],
  ];

  puntos.forEach(([circleId, labelId, cx, cy, label]) => {
    const circle = $(circleId);
    const text = $(labelId);
    if (!circle || !text) return;
    circle.setAttribute("cx", cx.toFixed(1));
    circle.setAttribute("cy", cy.toFixed(1));
    text.setAttribute("x", Math.min(cx + 9, 280).toFixed(1));
    text.setAttribute("y", Math.max(cy - 8, 18).toFixed(1));
    text.textContent = label;
  });
}

function limitar01(valor) {
  return clamp(Number(valor) || 0, 0, 1);
}

function clamp(valor, min, max) {
  return Math.min(max, Math.max(min, valor));
}

function nombreFeature(nombre) {
  const labels = {
    utilidad_neta: "Utilidad neta",
    frescura_book: "Frescura book",
    liquidez_fill: "Liquidez / fill",
    confiabilidad_ruta: "Confiabilidad",
    z_score_spread: "Z-score spread",
  };
  return labels[nombre] || nombre || "variable";
}

function renderLatencias(datos) {
  const container = $("latenciaRanking");
  if (!container) return;

  const latencias = [...(datos.latenciasExchange || [])]
    .sort((a, b) => (a.promedioMs || 0) - (b.promedioMs || 0))
    .slice(0, 6);

  if (latencias.length === 0) {
    if (container.children.length !== 1 || container.children[0].className !== "mini-empty") {
      container.innerHTML = '<p class="mini-empty">Esperando timestamps de feeds WebSocket.</p>';
    }
    return;
  }

  // Quitar el mensaje vacío si hay elementos
  if (container.children.length === 1 && container.children[0].className === "mini-empty") {
    container.innerHTML = "";
  }

  latencias.forEach((lat, index) => {
    let row = container.children[index];
    if (!row) {
      row = document.createElement("div");
      row.className = "latencia-row";
      row.innerHTML = '<strong class="latencia-exchange"></strong><span class="latencia-prom"></span><small class="latencia-percentiles"></small>';
      container.appendChild(row);
    }

    const estado = (lat.estado || "").includes("alta") ? "mala" : "buena";

    row.querySelector(".latencia-exchange").textContent = `#${index + 1} ${lat.exchange}`;
    const span = row.querySelector(".latencia-prom");
    span.className = `latencia-prom ${estado}`;
    span.textContent = `prom ${formato(lat.promedioMs || 0, 0)}ms`;
    row.querySelector(".latencia-percentiles").innerHTML = `<span class="latencia-label">p50</span> <strong class="latencia-valor">${formato(lat.p50Ms || 0, 0)}ms</strong> <span class="latencia-separador">|</span> <span class="latencia-label">p99</span> <strong class="latencia-valor ${estado}">${formato(lat.p99Ms || 0, 0)}ms</strong>`;
  });

  while (container.children.length > latencias.length) {
    container.removeChild(container.lastChild);
  }
}

function renderPipeline(pipeline, datos) {
  if (!pipeline) return;
  
  // Calcular Red (Transporte)
  let maxRedP50 = 0;
  let maxRedP95 = 0;
  let maxRedP99 = 0;
  if (datos && datos.latenciasExchange && datos.latenciasExchange.length > 0) {
    maxRedP50 = Math.max(...datos.latenciasExchange.map(l => l.p50Ms || 0));
    maxRedP95 = Math.max(...datos.latenciasExchange.map(l => l.p95Ms || 0));
    maxRedP99 = Math.max(...datos.latenciasExchange.map(l => l.p99Ms || 0));
  }
  const redEl = $("pipelineRed");
  if (redEl) setText("pipelineRed", `${formato(maxRedP50, 0)} ms`);
  const redTailEl = $("pipelineRedTail");
  if (redTailEl) setText("pipelineRedTail", `p95 ${formato(maxRedP95, 0)} ms · p99 ${formato(maxRedP99, 0)} ms`);

  const schedEl = $("pipelineScheduling");
  if (schedEl) setText("pipelineScheduling", `${formato(pipeline.schedulingP50Us || 0, 0)} µs`);
  const schedTailEl = $("pipelineSchedulingTail");
  if (schedTailEl) setText("pipelineSchedulingTail", `p95 ${formato(pipeline.schedulingP95Us || 0, 0)} µs · p99 ${formato(pipeline.schedulingP99Us || 0, 0)} µs`);

  const compEl = $("pipelineCompute");
  if (compEl) setText("pipelineCompute", `${formato(pipeline.computeP50Us || 0, 0)} µs`);
  const compTailEl = $("pipelineComputeTail");
  if (compTailEl) setText("pipelineComputeTail", `p95 ${formato(pipeline.computeP95Us || 0, 0)} µs · p99 ${formato(pipeline.computeP99Us || 0, 0)} µs`);

  setText("pipelineThroughput", `${formato(pipeline.eventosPorSegundo || 0, 1)} evt/s`);
  setText("pipelineRoutes", `${numero.format(pipeline.rutasEvaluadas || 0)} rutas evaluadas`);

  const rutas = Math.max(0, Number(pipeline.rutasEvaluadas || 0));
  const computeP50 = Math.max(0, Number(pipeline.computeP50Us || 0));
  const throughput = Math.max(0, Number(pipeline.eventosPorSegundo || 0));
  if (rutas > 0 || computeP50 > 0 || throughput > 0) {
    const partes = [];
    if (rutas > 0) partes.push(`${numero.format(rutas)} rutas evaluadas por este proceso`);
    if (computeP50 > 0) partes.push(`cómputo p50 de ${formato(computeP50, 0)} µs`);
    if (throughput > 0) partes.push(`${formato(throughput, 1)} eventos/s observados`);
    setText("landingRustMetrics", `${partes.join(" · ")}. La red espera en milisegundos; Mayab decide en microsegundos.`);
  }
}

let ablacionGACargada = false;
async function cargarAblacionGA() {
  if (ablacionGACargada) return;
  const tbody = $("gaAblationBody");
  if (!tbody) return;
  
  try {
    const res = await fetch("/api/ga/ablacion");
    if (!res.ok) throw new Error("Error en API");
    const data = await res.json();
    setText("gaSensitivityMethod", data.metodologia || "Train y holdout separados; configuración congelada antes de evaluar.");
    
    tbody.innerHTML = "";
    
    const filas = data.resultados || [];
    
    filas.forEach((d, index) => {
      const isActiva = index === filas.length - 1;
      const label = d.modelo || "Configuración";
      
      const tr = document.createElement("tr");
      if (isActiva) {
        tr.style.backgroundColor = "var(--card-bg)";
      }
      
      const pnlColor = d.medianaPnL > 0 ? "verde" : d.medianaPnL < 0 ? "rojo" : "";
      
      const fNum = (n) => `<td class="num">${n}</td>`;
      const fMoneda = (n, c) => `<td class="num ${c}">${dinero.format(n)}</td>`;
      const fPct = (n) => `<td class="num">${formato(n * 100, 1)}%</td>`;
      
      tr.innerHTML = `
        <td>${isActiva ? `<strong style="color:var(--morado)">${label}</strong>` : label}</td>
        ${fNum(formato(d.profitFactor, 2))}
        ${fPct(d.winRate)}
        ${fMoneda(-d.worstRunLoss, "")}
        ${fNum(d.runs)}
        ${fMoneda(d.medianaPnL, pnlColor)}
        <td class="num">${dinero.format(d.p05)} / ${dinero.format(d.p95)}</td>
      `;
      tbody.appendChild(tr);
    });
    
    ablacionGACargada = true;
  } catch (e) {
    debugError("No se pudo cargar la sensibilidad GA", e);
    tbody.innerHTML = `<tr><td colspan="7" class="text-center">Error al cargar sensibilidad GA</td></tr>`;
  }
}

function iniciarBacktest() {
  const btn = $("btnBacktest");
  if (!btn) return;
  let cargaEnCurso = false;
  const hacerBacktest = async () => {
    if (cargaEnCurso) return;
    cargaEnCurso = true;
    btn.disabled = true;
    btn.textContent = "Actualizando...";
    try {
      const res = await fetch("/api/backtest");
      if (res.ok) {
        renderBacktest(await res.json());
      }
    } catch (e) {
      debugError("Error ejecutando backtest", e);
    } finally {
      cargaEnCurso = false;
      btn.disabled = false;
      btn.textContent = "Actualizar";
    }
  };
  btn.onclick = hacerBacktest;
  btn.textContent = "Ejecutar backtest";
}

function iniciarResearchLab() {
  const btn = $("btnLabSweep");
  if (!btn) return;
  let cargaEnCurso = false;
  const hacerSweep = async () => {
    if (cargaEnCurso) return;
    cargaEnCurso = true;
    btn.disabled = true;
    btn.textContent = "Actualizando...";
    try {
      const res = await fetch("/api/lab/sweep");
      if (res.ok) renderLabSweep(await res.json());
    } catch (e) {
      debugError("Error ejecutando research lab", e);
    } finally {
      cargaEnCurso = false;
      btn.disabled = false;
      btn.textContent = "Actualizar";
    }
  };
  btn.onclick = hacerSweep;
  btn.textContent = "Comparar presets";
}

function iniciarEvidenceLab() {
  const boton = $("btnEvidenceReload");
  if (!boton) return;
  boton.addEventListener("click", cargarEvidenceLab);
}

async function cargarEvidenceLab() {
  const status = $("evidenceStatus");
  const grid = $("evidenceGrid");
  const boton = $("btnEvidenceReload");
  if (!status || !grid) return;
  boton.disabled = true;
  status.textContent = "Consultando contratos de solo lectura y economía de la última decisión…";
  const endpoints = [
    ["tapes", "/api/research/tapes"],
    ["walk", "/api/research/walk-forward"],
    ["impact", "/api/research/impact"],
    ["economics", "/api/research/economics"],
    ["execution", "/api/research/execution-matrix"],
    ["bootstrap", "/api/research/bootstrap"],
    ["microstructure", "/api/research/microstructure"],
    ["ou", "/api/research/ou"],
    ["ledger", "/api/research/ledger-audit"],
    ["readiness", "/api/readiness/live"],
  ];
  try {
    const respuestas = [];
    for (const [indice, [clave, url]] of endpoints.entries()) {
      status.textContent = `Consultando evidencia ${indice + 1}/${endpoints.length}: ${clave}…`;
      try {
        const res = await fetch(url);
        if (!res.ok) throw new Error(`HTTP ${res.status}`);
        respuestas.push([clave, url, await res.json(), null]);
      } catch (error) {
        respuestas.push([clave, url, null, error instanceof Error ? error.message : "no disponible"]);
      }
    }
    const evidencia = Object.fromEntries(respuestas.map(([clave, , data, error]) => [clave, data || { error } ]));
    const disponibles = respuestas.filter(([, , data]) => data).length;
    const tape = evidencia.tapes?.tapes?.[0];
    const wf = evidencia.walk?.gaVsBaselines || {};
    const principal = evidencia.bootstrap?.bootstrap?.principal?.deltasCandidatoMenosBaseline || {};
    const pnlCi = principal?.pnlNetoUsd?.ci95 || [];
    const impacto = evidencia.impact?.comparison || {};
    const ledger = evidencia.ledger || {};
    const micro = evidencia.microstructure?.report || {};
    const calibrators = micro.calibration || [];
    const winnerCalibration = calibrators.find((x) => x.model === micro.winnerByBrier);
    const ou = evidencia.ou?.report || {};
    const economics = evidencia.economics || {};
    const execution = evidencia.execution || {};
    const ledgerChecks = Object.values(ledger.checks || {});
    const ledgerOk = ledger.allPassed === true
      && ledgerChecks.length > 0
      && ledgerChecks.every((value) => value === true);
    const cards = [
      ["CORE · Proveniencia y hash del tape", tape
        ? `<strong>${escapeHtml(tape.provenance)}</strong><code>${escapeHtml(tape.sha256)}</code><small>${numero.format(tape.events || 0)} eventos · ${numero.format(tape.bytes || 0)} bytes</small>`
        : `<strong>No disponible</strong><small>${escapeHtml(evidencia.tapes?.error || "No hay tape montado")}</small>`, "/api/research/tapes"],
      ["CORE · Split train / calibration / holdout", `<strong>50% / 20% / 30%</strong><small>Campeón congelado: ${wf.campeonCongelado ? "sí" : "no"} · holdout no visto: ${wf.semillasHoldoutNoVistas?.length || 0} semillas</small>`, "/api/research/walk-forward"],
      ["CORE · GA vs baselines", `<strong>${escapeHtml(wf.ganador || "sin resultado")}</strong><small>${escapeHtml(wf.lectura || "Sin lectura disponible")}</small>`, "/api/research/walk-forward"],
      ["CORE · Comparación de impacto", `<strong>Default: ${escapeHtml(impacto.modeloPredeterminado || "—")}</strong><small>${numero.format(impacto.candidatos || 0)} candidatos pareados · menor error: ${escapeHtml(impacto.respuestas?.modeloConMenorErrorContraMarkout || "—")}</small>`, "/api/research/impact"],
      ["CORE · Bootstrap CI", `<strong>Δ PnL IC 95%: ${pnlCi.length === 2 ? `${dinero.format(pnlCi[0])} a ${dinero.format(pnlCi[1])}` : "no disponible"}</strong><small>${numero.format(evidencia.bootstrap?.bootstrap?.remuestras || 0)} remuestras · bloque principal ${numero.format(evidencia.bootstrap?.bootstrap?.bloquePrincipalSegundos || 0)} s</small>`, "/api/research/bootstrap"],
      ["EXPERIMENTAL · Microestructura", `<strong>${escapeHtml(micro.winnerByBrier || "sin resultado")}</strong><small>${numero.format(micro.observations || 0)} observaciones · ${escapeHtml(micro.sourceKind || "fuente desconocida")} · Brier ${formato(winnerCalibration?.brierScore || 0, 4)}</small><small>Research fuera del score core: quote age, OFI, markouts y Wilson 95%.</small>`, "/api/research/microstructure"],
      ["EXPERIMENTAL · OU fuera de muestra", `<strong>${escapeHtml(ou.decision || "sin resultado")}</strong><small>${numero.format(ou.observations || 0)} spreads · ${escapeHtml(ou.sourceKind || "fuente desconocida")} · ADF ${formato(ou.stationarity?.adfTStat || 0, 3)} · KPSS ${formato(ou.stationarity?.kpssStat || 0, 3)}</small><small>Research separado del GA y fuera del score core.</small>`, "/api/research/ou"],
      ["CORE · Auditoría del ledger", `<strong>${ledgerOk ? "Checks del snapshot pasan" : "Revisión requerida"}</strong><code>${escapeHtml(ledger.snapshotSha256 || "sin hash")}</code><small>${numero.format(ledger.counts?.operations || 0)} operaciones · ${numero.format(ledger.counts?.decisionAudits || 0)} decisiones auditadas</small>`, "/api/research/ledger-audit"],
      ["CORE · Matriz forense de ejecución", `<strong>${execution.allPassed ? `${numero.format(execution.passed || 0)}/${numero.format(execution.total || 12)} escenarios conciliados` : "Revisión requerida"}</strong><code>${escapeHtml(execution.matrixSha256 || "sin hash")}</code><small>fills parciales · timeouts · duplicados · restart · retry · re-hedge/unwind · 10 invariantes por caso</small>`, "/api/research/execution-matrix"],
      ["ALCANCE · Limitaciones conocidas", `<strong>${escapeHtml(evidencia.readiness?.status || "estado desconocido")}</strong><ul>${(evidencia.readiness?.limitations || []).map(x => `<li>${escapeHtml(x)}</li>`).join("")}</ul>`, "/api/readiness/live"],
    ];
    grid.innerHTML = `${renderEconomicsEvidence(economics)}${cards.map(([titulo, contenido, url]) => `<article class="evidence-lab-card"><h3>${escapeHtml(titulo)}</h3>${contenido}<a class="btn-link evidence-artifact-link" href="${url}" target="_blank" rel="noopener">Abrir artefacto JSON</a></article>`).join("")}`;
    grid.querySelector("[data-run-jury]")?.addEventListener("click", prepararDemoFinal);
    status.textContent = `${disponibles} de ${endpoints.length} contratos respondieron. Cada ausencia queda aislada; los demás resultados siguen siendo evaluables.`;
  } catch (error) {
    status.textContent = `No se pudo completar Evidence Lab: ${error.message}`;
    grid.innerHTML = "";
    debugError("Evidence Lab", error);
  } finally {
    boton.disabled = false;
  }
}

function renderEconomicsEvidence(economics) {
  if (!economics?.available) {
    return `<article class="evidence-lab-card economics-unavailable"><h3>CORE · Economía de la decisión</h3><strong>Aún no hay una operación para analizar</strong><small>${escapeHtml(economics?.reason || economics?.error || "Ejecuta la prueba completa para generar una operación con costos.")}</small><button type="button" class="btn-link" data-run-jury>Ejecutar prueba completa</button></article>`;
  }

  const waterfall = economics.edgeWaterfall || {};
  const waterfallItems = waterfall.items || [];
  const waterfallMax = Math.max(...waterfallItems.map((item) => Math.abs(Number(item.valueUsd || 0))), 0.01);
  const waterfallHtml = waterfallItems.map((item) => {
    const value = Number(item.valueUsd || 0);
    const width = Math.max(2, Math.abs(value) / waterfallMax * 100);
    return `<li class="${value >= 0 ? "positive" : "negative"}"><span>${escapeHtml(item.label || item.key)}</span><i><b style="width:${width}%"></b></i><strong>${dinero.format(value)}</strong></li>`;
  }).join("");

  const frontier = economics.breakEvenFrontier || {};
  const frontierPct = Math.max(0, Math.min(100, Number(frontier.currentTotalCostUsd || 0) / Math.max(Number(frontier.maxTotalCostUsd || 0), 0.01) * 100));
  const points = economics.capacityCurve?.points || [];
  const maxCapacityPnl = Math.max(...points.map((point) => Math.abs(Number(point.expectedPnlUsd || 0))), 0.01);
  const capacityHtml = points.map((point) => {
    const pnl = Number(point.expectedPnlUsd || 0);
    const width = Math.max(2, Math.abs(pnl) / maxCapacityPnl * 100);
    return `<li class="${pnl >= 0 ? "positive" : "negative"}${point.insideObservedCapacity ? "" : " extrapolated"}"><span>${btc.format(point.quantityBtc || 0)} BTC</span><i><b style="width:${width}%"></b></i><strong>${dinero.format(pnl)}</strong></li>`;
  }).join("");

  const stages = economics.decisionFunnel?.stages || [];
  const funnelBase = Math.max(Number(stages[0]?.count || 0), 1);
  const funnelHtml = stages.map((stage) => {
    const count = Number(stage.count || 0);
    const width = Math.max(count > 0 ? 4 : 0, count / funnelBase * 100);
    return `<li><span>${escapeHtml(stage.label || stage.key)}</span><i><b style="width:${width}%"></b></i><strong>${numero.format(count)}</strong></li>`;
  }).join("");

  return `
    <article class="evidence-lab-card economics-card economics-waterfall">
      <h3>CORE · Desglose de utilidad</h3>
      <small>${escapeHtml(economics.source?.route || "ruta")}: efecto de cada costo sobre el spread.</small>
      <ol>${waterfallHtml}</ol>
      <strong class="economics-verdict">${waterfall.reconciledWithinCent ? "El desglose coincide con el ledger" : "Revisar conciliación"}</strong>
      <a class="btn-link evidence-artifact-link" href="/api/research/economics" target="_blank" rel="noopener">Abrir datos</a>
    </article>
    <article class="evidence-lab-card economics-card economics-frontier">
      <h3>CORE · Punto de equilibrio</h3>
      <small>Costo adicional que puede absorber la ruta antes de perder utilidad.</small>
      <div class="frontier-track" aria-label="Costo actual contra costo de break-even"><b style="width:${frontierPct}%"></b></div>
      <dl><div><dt>Costo actual</dt><dd>${dinero.format(frontier.currentTotalCostUsd || 0)}</dd></div><div><dt>Break-even</dt><dd>${dinero.format(frontier.maxTotalCostUsd || 0)}</dd></div><div><dt>Headroom</dt><dd>${formato(frontier.additionalCostHeadroomBps || 0, 2)} bps</dd></div></dl>
      <a class="btn-link evidence-artifact-link" href="/api/research/economics" target="_blank" rel="noopener">Abrir datos</a>
    </article>
    <article class="evidence-lab-card economics-card economics-capacity">
      <h3>CORE · Curva de capacidad</h3>
      <small>PnL esperado por tamaño. El tramado indica valores fuera de la profundidad observada.</small>
      <ol>${capacityHtml}</ol>
      <a class="btn-link evidence-artifact-link" href="/api/research/economics" target="_blank" rel="noopener">Abrir datos</a>
    </article>
    <article class="evidence-lab-card economics-card economics-funnel">
      <h3>CORE · Filtro de decisiones</h3>
      <small>Rutas que sobreviven frescura, profundidad, costos y riesgo.</small>
      <ol>${funnelHtml}</ol>
      <a class="btn-link evidence-artifact-link" href="/api/research/economics" target="_blank" rel="noopener">Abrir datos</a>
    </article>`;
}

function renderBacktest(datos) {
  const tbody = $("backtestResultados");
  if (!tbody) return;
  tbody.textContent = "";
  [
    ["Base", datos.base],
    ["Optimizada", datos.optimizada],
  ].forEach(([nombre, r]) => {
    const tr = document.createElement("tr");
    [nombre, numero.format(r.tradesEjecutados), dinero.format(r.pnlUsd), `${formato(r.winRate * 100, 1)}%`, dinero.format(r.maxDrawdownUsd), `±${dinero.format(r.intervaloConfianza95Usd || 0)}`, formato(r.profitFactor || 0, 2)]
      .forEach((valor, i) => {
        const td = document.createElement("td");
        td.textContent = valor;
        if (i === 2) td.className = r.pnlUsd >= 0 ? "positivo" : "negativo";
        tr.appendChild(td);
      });
    tbody.appendChild(tr);
  });
  const evidencia = $("backtestEvidencia");
  const validacion = datos?.validacionMultisemilla;
  if (evidencia && validacion) {
    const base = validacion.base || {};
    const optimizada = validacion.optimizada || {};
    const delta = Number(validacion.deltaPnlMedianoUsd || 0);
    const holdout = datos?.validacionFueraMuestra || {};
    const bootstrap = datos?.significanciaBootstrap || {};
    const principal = bootstrap.principal || {};
    const deltaPnl = principal?.deltasCandidatoMenosBaseline?.pnlNetoUsd || {};
    const deltaDd = principal?.deltasCandidatoMenosBaseline?.maxDrawdownUsd || {};
    const ciPnl = Array.isArray(deltaPnl.ci95) ? deltaPnl.ci95 : [0, 0];
    const probabilidad = Number(principal.probabilidadDeltaPnlMayorCero || 0);
    const estable = bootstrap.estabilidadVentanas || {};
    const concluyente = principal.resultado !== "resultado inconcluso";
    evidencia.classList.toggle("evidence-positive", concluyente && ciPnl[0] > 0);
    evidencia.innerHTML = `
      <strong>${escapeHtml(principal.resultado || (holdout.gaGana ? "El campeón GA gana fuera de muestra" : "El baseline gana fuera de muestra"))}</strong>
      <span>Δ P&amp;L mediano ${dinero.format(deltaPnl.mediana || 0)} · Bootstrap 95% CI [${dinero.format(ciPnl[0] || 0)}, ${dinero.format(ciPnl[1] || 0)}] · P(ΔPnL &gt; 0) ${formato(probabilidad * 100, 1)}%</span>
      <small>Δ drawdown mediano ${dinero.format(deltaDd.mediana || 0)} · estable en ${numero.format(estable.favorables || 0)}/${numero.format(estable.ventanas || 5)} ventanas · ${numero.format(bootstrap.remuestras || 0)} remuestras pareadas, bloques de ${numero.format(bootstrap.bloquePrincipalSegundos || 0)} s.</small>
      <small>${escapeHtml(holdout.lectura || validacion.lectura || "Comparación reproducible multisemilla.")}</small>
      <small>Control: ${numero.format(base.corridas || 0)} semillas · Δ mediano previo ${dinero.format(delta)} · GA positivo en ${numero.format(optimizada.corridasPnlPositivo || 0)}/${numero.format(optimizada.corridas || 0)} corridas.</small>
    `;
  }
  renderComparacionImpacto(datos?.comparacionImpacto);
  aplicarFiltroTabla(tbody.closest("table"));
}

function renderComparacionImpacto(comparacion) {
  const tbody = $("impactoResultados");
  if (!tbody) return;
  tbody.textContent = "";
  (comparacion?.tabla || []).forEach((r) => {
    const tr = document.createElement("tr");
    [
      r.modelo || "Modelo",
      dinero.format(r.pnlUsd || 0),
      `${formato((r.fillRate || 0) * 100, 1)}%`,
      dinero.format(r.maxDrawdownUsd || 0),
      `${formato(r.impactoMedioBps || 0, 2)} bps`,
      numero.format(r.decisionesDistintas || 0),
    ].forEach((valor, i) => {
      const td = document.createElement("td");
      td.textContent = valor;
      if (i === 1) td.className = (r.pnlUsd || 0) >= 0 ? "positivo" : "negativo";
      tr.appendChild(td);
    });
    tbody.appendChild(tr);
  });
  const evidencia = $("impactoEvidencia");
  const respuestas = comparacion?.respuestas || {};
  if (evidencia) {
    evidencia.innerHTML = `
      <strong>Default: ${escapeHtml(comparacion?.modeloPredeterminado || "Book-walk")}</strong>
      <span>${numero.format(respuestas.bookWalkAceptaSquareRootRechaza || 0)} oportunidades acepta book-walk y rechaza square-root.</span>
      <small>Menor error contra markout: ${escapeHtml(respuestas.modeloConMenorErrorContraMarkout || "sin datos")}. Mayor subestimación ex post: ${escapeHtml(respuestas.modeloQueMasSubestimaCostoExPost || "sin datos")}.</small>
    `;
  }
  aplicarFiltroTabla(tbody.closest("table"));
}

function renderLabSweep(datos) {
  const tbody = $("labSweepResultados");
  if (!tbody) return;
  tbody.textContent = "";
  setText("labLectura", datos?.lectura || "Sweep reproducible cargado.");
  const ganador = datos?.ganador;
  (datos?.resultados || []).forEach((row) => {
    const r = row.resultado || {};
    const tr = document.createElement("tr");
    if (row.preset === ganador) tr.className = "fila-seleccionada";
    [
      row.preset || "preset",
      `${formato(row.umbralBps || 0, 2)} bps`,
      `${formato(row.maxOperacionBtc || 0, 3)} BTC`,
      numero.format(r.tradesEjecutados || 0),
      dinero.format(r.pnlUsd || 0),
      dinero.format(r.maxDrawdownUsd || 0),
      `${formato((r.winRate || 0) * 100, 1)}%`,
      formato(row.scoreLab || 0, 2),
    ].forEach((valor, i) => {
      const td = document.createElement("td");
      td.textContent = valor;
      if (i === 4) td.className = (r.pnlUsd || 0) >= 0 ? "positivo" : "negativo";
      tr.appendChild(td);
    });
    tbody.appendChild(tr);
  });
  const evidencia = $("labEvidencia");
  const filaGa = (datos?.resultados || []).find((row) => row.preset === "ga_edge");
  const filaBase = (datos?.resultados || []).find((row) => row.preset === "balanceado");
  if (evidencia && filaGa?.validacion && filaBase?.validacion) {
    const delta = Number(filaGa.validacion.pnlMedianoUsd || 0) - Number(filaBase.validacion.pnlMedianoUsd || 0);
    evidencia.classList.toggle("evidence-positive", delta >= 0);
    evidencia.innerHTML = `
      <strong>Robustez, no una semilla afortunada</strong>
      <span>GA vs balanceado: Δ PnL mediano ${dinero.format(delta)} en ${numero.format(filaGa.validacion.corridas || 0)} semillas comunes.</span>
      <small>P05–P95 GA: ${dinero.format(filaGa.validacion.pnlP05Usd || 0)} a ${dinero.format(filaGa.validacion.pnlP95Usd || 0)}. ${escapeHtml(datos.limitacion || "")}</small>
    `;
  }
  aplicarFiltroTabla(tbody.closest("table"));
}

function renderBalances(datos) {
  const container = $("balances");
  if (!container) return;
  container.textContent = "";

  const ordenados = [...datos.balances].sort((a, b) => a.exchange.localeCompare(b.exchange));
  ordenados.forEach((b) => {
    const div = document.createElement("div");
    div.className = "balance";
    const strong = document.createElement("strong");
    strong.textContent = b.exchange;
    const span = document.createElement("span");
    span.innerHTML = `${dinero.format(b.usd)}<br>${btc.format(b.btc)} BTC`;
    div.appendChild(strong);
    div.appendChild(span);
    container.appendChild(div);
  });

  const divCost = document.createElement("div");
  divCost.className = "balance balance-total";
  divCost.innerHTML = `<strong>Costos Reb. (acum)</strong><span style="color:var(--rojo)">${dinero.format(datos?.metricas?.costoRebalanceoAcumuladoUsd || 0)}</span>`;
  container.appendChild(divCost);
}

function renderTransferencias(datos) {
  const tbody = $("tablaTransferencias");
  const countSpan = $("transferenciasCount");
  if (!tbody) return;

  const transferencias = datos.transferenciasInventario || [];
  
  if (transferencias.length === 0) {
    tbody.innerHTML = `<tr><td colspan="7" class="text-center">Sin transferencias recientes</td></tr>`;
    if (countSpan) countSpan.textContent = "0 pendientes";
    return;
  }
  
  let pendientesCount = 0;
  
  const df = document.createDocumentFragment();
  transferencias.forEach((t) => {
    if (["TRANSFER_REQUESTED", "IN_TRANSIT", "CONFIRMED"].includes(t.estado)) pendientesCount++;
    const tr = document.createElement("tr");
    
    let color = t.estado === "FAILED" ? "var(--rojo)" : t.estado === "AVAILABLE" ? "var(--verde)" : "var(--amarillo)";
    
    tr.innerHTML = `
      <td><span style="color: ${color}; font-weight: bold; text-transform: lowercase;">${escapeHtml(t.estado)}</span></td>
      <td><strong>${escapeHtml(t.desde)}</strong> &rarr; <strong>${escapeHtml(t.hacia)}</strong></td>
      <td>${escapeHtml(t.activo)}</td>
      <td class="text-right">${t.activo === "BTC" ? btc.format(t.cantidadBruta) : dinero.format(t.cantidadBruta)}</td>
      <td class="text-right">${t.activo === "BTC" ? btc.format(t.cantidadNeta) : dinero.format(t.cantidadNeta)}</td>
      <td class="text-right">${dinero.format(t.costoUsd)}</td>
      <td class="text-right" title="Red ${escapeHtml(t.redElegida || "n/a")} · confirmaciones ${numero.format(t.confirmacionesObservadas || 0)}/${numero.format(t.confirmacionesRequeridas || 0)} · mínimo ${numero.format(t.minimoRetiro || 0)} (${t.cumpleMinimo ? "cumple" : "no cumple"}) · retiro ${t.retiroSuspendido ? "suspendido" : "disponible"} · capital bloqueado ${dinero.format(t.capitalBloqueadoUsd || 0)} · prob. demora ${formato((t.probabilidadDemora || 0) * 100, 1)}% · ETA ${numero.format(t.etaMs || 0)} ms · retraso ${numero.format(t.retrasoSimuladoMs || 0)} ms · costo oportunidad ${dinero.format(t.costoOportunidadUsd || 0)} · capacidad ${numero.format(t.capacidadOperativaRestante || 0)}"><small>${formatearHoraLocal(t.liquidaEn)}</small></td>
    `;
    df.appendChild(tr);
  });
  
  tbody.textContent = "";
  tbody.appendChild(df);
  
  if (countSpan) {
    countSpan.textContent = `${pendientesCount} pendiente${pendientesCount === 1 ? '' : 's'}`;
  }
}


function renderConfig(datos) {
  const container = $("configGrid");
  if (!container) return;
  container.textContent = "";

  const c = datos.configuracion;
  const items = [
    { label: "Máx. operación", val: `${btc.format(c.maxOperacionBtc)} BTC` },
    { label: "Diferencial mínimo", val: `${formato(c.minDiferencialNetoBps, 2)} bps` },
    { label: "Deslizamiento", val: `${formato(c.deslizamientoBps, 2)} bps` },
    { label: "Enfriamiento", val: `${c.enfriamientoMs} ms` },
    { label: "Latencia riesgo", val: `${formato(c.latenciaRiesgoBps, 2)} bps` },
    { label: "Retiro amort.", val: `${formato(c.retiroAmortizadoBps, 2)} bps` },
    { label: "Basis USDT/USD", val: `${formato(c.usdtUsdPremiumBps, 2)} bps` },
    { label: "Circuit breaker", val: `${dinero.format(c.circuitBreakerPerdidaUsd)} / ${c.circuitBreakerVentanaMin} min` },
    { label: "Volatilidad", val: `${formato(c.volatilidadUmbralBps, 1)} bps / ${c.volatilidadVentanaSeg}s` },
    { label: "Adversidad", val: c.simularAdversidad ? "activa" : "apagada" },
    { label: "Cruce USD/USDT", val: c.permitirCruceUsdUsdt ? "permitido" : "separado" },
    { label: "Rebalanceo", val: `${formato(c.rebalanceUmbralPct, 0)}% · ${dinero.format(c.costoRebalanceoUsd || 0)}` },
  ];

  items.forEach((item) => {
    const div = document.createElement("div");
    const span = document.createElement("span");
    span.textContent = item.label;
    const strong = document.createElement("strong");
    strong.textContent = item.val;
    div.appendChild(span);
    div.appendChild(strong);
    container.appendChild(div);
  });
}

/**
 * @description Renderiza la tabla de oportunidades detectadas, ordenándolas por score
 * y permitiendo la visualización detallada al hacer clic.
 * @param {Object} datos - El estado público actual.
 */
function renderOportunidades(datos) {
  const tbody = $("oportunidades");
  if (!tbody) return;
  tbody.textContent = "";
  const oportunidades = oportunidadesVigentes(datos);
  const instanteVigente = oportunidades[0]?.detectadaEn;
  const scorePorRuta = new Map();
  (datos.auditoriaDecisiones || []).forEach((a) => {
    if (instanteVigente && a.tiempo !== instanteVigente) return;
    if (!scorePorRuta.has(a.ruta)) {
      scorePorRuta.set(a.ruta, a.score || 0);
    }
  });

  oportunidades.slice(0, 16).forEach((o) => {
    const tr = document.createElement("tr");
    tr.className = o.id === oportunidadSeleccionadaId ? "fila-seleccionada" : "";
    tr.tabIndex = 0;
    tr.addEventListener("click", () => {
      oportunidadSeleccionadaId = o.id;
      renderDetalleOportunidad(ultimoEstado);
      renderOportunidades(ultimoEstado);
    });
    tr.addEventListener("keydown", (evento) => {
      if (evento.key === "Enter" || evento.key === " ") {
        evento.preventDefault();
        tr.click();
      }
    });

    const tdRuta = document.createElement("td");
    let ruta = escapeHtml(`${o.compraEn} -> ${o.ventaEn}`);
    if (o.tipo === "Triangular") {
      ruta = `<span class="badge-triangular" title="Arbitraje triangular">TRI</span> ${ruta}`;
    }
    tdRuta.innerHTML = ruta;

    const tdNeto = document.createElement("td");
    tdNeto.className = o.diferencialNetoBps >= 0 ? "positivo" : "negativo";
    tdNeto.textContent = `${formato(o.diferencialNetoBps, 2)} bps`;

    const tdZScore = document.createElement("td");
    tdZScore.textContent = formato(o.zScore, 2);

    const tdScore = document.createElement("td");
    const score = scorePorRuta.get(`${o.compraEn}->${o.ventaEn}`) || 0;
    tdScore.textContent = score ? formato(score, 3) : "—";

    const tdAmt = document.createElement("td");
    tdAmt.textContent = btc.format(o.cantidadBtc);

    const tdProfit = document.createElement("td");
    tdProfit.textContent = dinero.format(o.utilidadUsd);

    const tdStatus = document.createElement("td");
    const chip = document.createElement("span");
    chip.className = o.ejecutable ? "chip-ok" : "chip-no";
    chip.textContent = o.ejecutable ? "ejecutable" : o.razon;
    tdStatus.appendChild(chip);

    tr.appendChild(tdRuta);
    tr.appendChild(tdNeto);
    tr.appendChild(tdZScore);
    tr.appendChild(tdScore);
    tr.appendChild(tdAmt);
    tr.appendChild(tdProfit);
    tr.appendChild(tdStatus);
    tbody.appendChild(tr);
  });
}

function renderDetalleOportunidad(datos) {
  const panel = $("detalleOportunidad");
  if (!panel) return;
  const oportunidades = oportunidadesVigentes(datos);
  const seleccionada = oportunidades.find((o) => o.id === oportunidadSeleccionadaId) || oportunidades[0];
  if (!seleccionada) {
    panel.innerHTML = `
      <span class="ceja">Forense</span>
      <strong>Selecciona una decisión</strong>
      <p>La fila elegida mostrará utilidad neta, costos, liquidez, latencia, inventario y el motivo exacto del motor.</p>
    `;
    return;
  }
  if (oportunidadSeleccionadaId !== seleccionada.id) {
    oportunidadSeleccionadaId = seleccionada.id;
  }
  const costos = seleccionada.costos || {};
  const estado = seleccionada.ejecutable ? "Ejecutable" : seleccionada.razon;
  const auditoria = (datos?.auditoriaDecisiones || []).find((a) =>
    a.ruta === `${seleccionada.compraEn}->${seleccionada.ventaEn}` &&
    a.tiempo === seleccionada.detectadaEn
  );
  const score = auditoria?.score || 0;
  const decisionCode = auditoria?.decisionCode || seleccionada.decisionCode || "NO_CODE";
  const decisionReason = auditoria?.decisionReason || seleccionada.decisionReason || seleccionada.razon || "";
  const decisionActual = auditoria?.decisionActual ?? seleccionada.decisionActual;
  const decisionThreshold = auditoria?.decisionThreshold ?? seleccionada.decisionThreshold;
  const esTriangular = seleccionada.tipo === "Triangular";
  const badgeTriangular = esTriangular ? `<span class="badge-triangular" title="Arbitraje triangular">TRI</span>` : "";

  let piernasHtml = "";
  if (esTriangular && seleccionada.piernas) {
    piernasHtml = `
      <div class="piernas-stack">
        <span class="ceja">Piernas del Ciclo</span>
        ${seleccionada.piernas.map(p => `
          <div class="pierna">
            <span>${escapeHtml(p.exchange)} (${escapeHtml(p.par)})</span>
            <strong>${escapeHtml(p.accion)} @ ${Number.isFinite(p.precio) ? formato(p.precio, 2) : "—"}</strong>
          </div>
        `).join("")}
      </div>
    `;
  }

  panel.innerHTML = `
    <div class="detalle-header">
      <span class="ceja">Forense</span>
      <strong>${badgeTriangular} ${escapeHtml(seleccionada.compraEn)} -> ${escapeHtml(seleccionada.ventaEn)}</strong>
      <span class="${seleccionada.ejecutable ? "chip-ok" : "chip-no"}">${escapeHtml(estado)}</span>
    </div>
    ${piernasHtml}

    <div class="detalle-grid">
      <div><span>Bruto</span><strong>${formato(seleccionada.diferencialBrutoBps, 2)} bps</strong></div>
      <div><span>Neto</span><strong>${formato(seleccionada.diferencialNetoBps, 2)} bps</strong></div>
      <div><span>Tamaño</span><strong>${btc.format(seleccionada.cantidadBtc)} BTC</strong></div>
      <div><span>Utilidad</span><strong>${dinero.format(seleccionada.utilidadUsd)}</strong></div>
      <div><span>Latencia</span><strong>${seleccionada.latenciaMaxMs} ms</strong></div>
      <div><span>Z-Score</span><strong>${formato(seleccionada.zScore, 2)}</strong></div>
      <div><span>Score EV</span><strong>${score ? formato(score, 3) : "sin score"}</strong></div>
      <div class="decision-code-cell"><span>Código</span><span class="decision-code-badge ${esDecisionOk(decisionCode) ? 'ok' : 'bad'}">${escapeHtml(decisionCode)}</span></div>
      <div><span>Actual</span><strong>${Number.isFinite(decisionActual) ? formato(decisionActual, 2) : "—"}</strong></div>
      <div><span>Umbral</span><strong>${Number.isFinite(decisionThreshold) ? formato(decisionThreshold, 2) : "—"}</strong></div>
    </div>
    <div class="cost-stack">
      <div><span>Fee compra</span><strong>${dinero.format(costos.feeCompraUsd || 0)}</strong></div>
      <div><span>Fee venta</span><strong>${dinero.format(costos.feeVentaUsd || 0)}</strong></div>
      <div><span>Slippage</span><strong>${dinero.format(costos.deslizamientoUsd || 0)}</strong></div>
      <div><span>Retiro amort.</span><strong>${dinero.format(costos.retiroAmortUsd || 0)}</strong></div>
      <div><span>Riesgo latencia</span><strong>${dinero.format(costos.latenciaRiesgoUsd || 0)}</strong></div>
      <div><span>Selección adversa</span><strong>${dinero.format(costos.seleccionAdversaUsd || 0)}</strong></div>
      <div><span>Total costos</span><strong>${dinero.format(costos.totalUsd || 0)}</strong></div>
    </div>
    <p class="decision-reason">${escapeHtml(decisionReason)}</p>
  `;
}

function esDecisionOk(code) {
  return /^(ACCEPT|DEMO_PROFITABLE|PARTIAL_FILL)/.test(String(code || ""));
}

function renderOperaciones(datos) {
  const tbody = $("operaciones");
  if (!tbody) return;
  tbody.textContent = "";

  datos.operaciones.slice(0, 16).forEach((o) => {
    const tr = document.createElement("tr");

    const tdBuy = document.createElement("td");
    const esTriangular = o.tipo === "Triangular";
    const badgeTri = esTriangular ? `<span class="badge-triangular" title="Arbitraje triangular">TRI</span> ` : "";
    tdBuy.innerHTML = `${badgeTri}${escapeHtml(o.compraEn)}<br><span>${dinero.format(o.precioCompra)}</span>`;

    const tdSell = document.createElement("td");
    tdSell.innerHTML = `${escapeHtml(o.ventaEn)}<br><span>${dinero.format(o.precioVenta)}</span>`;

    const tdAmt = document.createElement("td");
    tdAmt.textContent = btc.format(o.cantidadBtc);

    const tdProfit = document.createElement("td");
    tdProfit.className = o.utilidadUsd >= 0 ? "positivo" : "negativo";
    tdProfit.textContent = dinero.format(o.utilidadUsd);

    const tdLat = document.createElement("td");
    tdLat.textContent = `${o.latenciaMaxMs} ms`;

    tr.appendChild(tdBuy);
    tr.appendChild(tdSell);
    tr.appendChild(tdAmt);
    tr.appendChild(tdProfit);
    tr.appendChild(tdLat);
    tbody.appendChild(tr);
  });
}

function renderEventosEjecucion(datos) {
  const tbody = $("eventosEjecucion");
  if (!tbody) return;
  tbody.textContent = "";

  (datos.eventosEjecucion || []).slice(0, 14).forEach((e) => {
    const tr = document.createElement("tr");

    const tdTipo = document.createElement("td");
    const chip = document.createElement("span");
    chip.className = e.severidad === "alta" ? "chip-bad" : e.severidad === "media" ? "chip-warn" : "chip-ok";
    chip.textContent = e.tipo;
    tdTipo.appendChild(chip);

    const tdRuta = document.createElement("td");
    tdRuta.textContent = e.ruta;

    const tdDetalle = document.createElement("td");
    tdDetalle.textContent = e.detalle;

    const tdProfit = document.createElement("td");
    tdProfit.className = e.utilidadUsd >= 0 ? "positivo" : "negativo";
    tdProfit.textContent = dinero.format(e.utilidadUsd || 0);

    tr.appendChild(tdTipo);
    tr.appendChild(tdRuta);
    tr.appendChild(tdDetalle);
    tr.appendChild(tdProfit);
    tbody.appendChild(tr);
  });
}

function renderTrazasEjecucion(datos) {
  const tbody = $("trazasEjecucion");
  if (!tbody) return;
  tbody.textContent = "";
  const trazas = (datos.trazasEjecucion || []).slice(0, 28);
  if (trazas.length === 0) {
    const tr = document.createElement("tr");
    tr.innerHTML = '<td colspan="6" class="vacio">Prepara la demo auditada o ejecuta “Fallar segunda pierna + unwind”.</td>';
    tbody.appendChild(tr);
    return;
  }
  trazas.forEach((trace) => {
    const tr = document.createElement("tr");
    const estado = document.createElement("td");
    estado.innerHTML = `<span class="decision-code-badge ${String(trace.estado || "").includes("FAILED") || String(trace.estado || "").includes("LOSS") ? "bad" : "ok"}">${escapeHtml(trace.estado || "—")}</span>`;
    const pierna = document.createElement("td");
    pierna.textContent = trace.pierna || "—";
    const ruta = document.createElement("td");
    ruta.textContent = trace.ruta || "—";
    const exposicion = document.createElement("td");
    exposicion.textContent = `${btc.format(trace.exposicionBtc || 0)} BTC`;
    const pnl = document.createElement("td");
    pnl.textContent = dinero.format(trace.pnlRealizadoUsd || 0);
    pnl.className = Number(trace.pnlRealizadoUsd || 0) < 0 ? "negativo" : "";
    const detalle = document.createElement("td");
    detalle.textContent = trace.detalle || `${trace.estadoAnterior || "—"} → ${trace.estado || "—"}`;
    tr.append(estado, pierna, ruta, exposicion, pnl, detalle);
    tbody.appendChild(tr);
  });
}

function renderRebalanceos(datos) {
  const tbody = $("rebalanceos");
  if (!tbody) return;
  tbody.textContent = "";

  (datos.rebalanceos || []).slice(0, 14).forEach((r) => {
    const tr = document.createElement("tr");
    const tdActivo = document.createElement("td");
    tdActivo.textContent = r.activo;
    const tdDesde = document.createElement("td");
    tdDesde.textContent = r.desde;
    const tdHacia = document.createElement("td");
    tdHacia.textContent = r.hacia;
    const tdCantidad = document.createElement("td");
    tdCantidad.textContent = r.activo === "BTC" ? `${btc.format(r.cantidad)} BTC` : dinero.format(r.cantidad);
    const tdCosto = document.createElement("td");
    tdCosto.textContent = dinero.format(r.costoUsd || 0);
    tr.appendChild(tdActivo);
    tr.appendChild(tdDesde);
    tr.appendChild(tdHacia);
    tr.appendChild(tdCantidad);
    tr.appendChild(tdCosto);
    tbody.appendChild(tr);
  });
}

function renderAuditoriaDecisiones(datos) {
  const tbody = $("auditoriaDecisiones");
  if (!tbody) return;
  tbody.textContent = "";

  (datos.auditoriaDecisiones || []).slice(0, 18).forEach((a) => {
    const tr = document.createElement("tr");

    const tdRuta = document.createElement("td");
    tdRuta.innerHTML = `${escapeHtml(a.ruta)}<br><span>${escapeHtml(a.par || "")}</span>`;

    const tdDecision = document.createElement("td");
    const chip = document.createElement("span");
    chip.className = a.decision === "candidata_ejecutable" ? "chip-ok" : "chip-no";
    chip.textContent = a.decision === "candidata_ejecutable" ? "acepta" : "descarta";
    tdDecision.appendChild(chip);

    const tdCodigo = document.createElement("td");
    const codigo = document.createElement("code");
    codigo.className = "decision-code";
    codigo.textContent = a.decisionCode || "NO_CODE";
    tdCodigo.appendChild(codigo);

    const tdScore = document.createElement("td");
    tdScore.textContent = formato(a.score || 0, 4);

    const tdCosto = document.createElement("td");
    tdCosto.textContent = dinero.format(a.costoTotalUsd || 0);

    const tdRazon = document.createElement("td");
    const pesos = (a.pesosGa || []).map((p) => formato(p * 100, 0)).join("/");
    const actual = Number.isFinite(a.decisionActual) ? formato(a.decisionActual, 2) : "—";
    const umbral = Number.isFinite(a.decisionThreshold) ? formato(a.decisionThreshold, 2) : "—";
    tdRazon.textContent = `${a.decisionReason || a.razon || "sin razón"} · actual ${actual} / umbral ${umbral} · ${formato(a.diferencialNetoBps || 0, 2)} bps · pesos ${pesos}`;

    tr.appendChild(tdRuta);
    tr.appendChild(tdDecision);
    tr.appendChild(tdCodigo);
    tr.appendChild(tdScore);
    tr.appendChild(tdCosto);
    tr.appendChild(tdRazon);
    tbody.appendChild(tr);
  });
}

function renderGenetico(datos) {
  const g = datos.genetico;
  if (!g) return;
  const validacion = g.validacion || {};
  const sha = typeof validacion.datasetHash === "string" && validacion.datasetHash.length > 12
    ? `${validacion.datasetHash.slice(0, 12)}…`
    : validacion.datasetHash || "sin huella";

  const genEl = $("gaGeneracion");
  if (genEl) genEl.textContent = `Gen ${g.generacion} · ${g.poblacion} ind.`;
  setText("gaEstado", g.activo ? "Optimizando estrategia" : "Listo para competir");
  setText(
    "gaValidacion",
    `${validacion.campeon || "ga_offline_v1"} vs ${validacion.challenger || "ga_live_explorer"} · ${validacion.holdoutSellado ? "holdout sellado" : "holdout abierto"} · ${validacion.semillasEntrenamiento || 0}/${validacion.semillasHoldout || 0} semillas · SHA ${sha}`,
  );
  setText("gaMuestras", `${numero.format(g.operacionesEvaluadas || 0)} ops · ${numero.format(g.fallosEvaluados || 0)} fallos`);
  setText("gaUmbral", `${formato(g.umbralOptimizado, 2)} bps`);
  setText("gaMaxBtc", `${formato(g.maxOperacionOptimizadaBtc, 3)} BTC`);
  setText("gaLatencia", `${numero.format(g.toleranciaLatenciaMs || 0)} ms`);
  setText("gaMejora", `+${formato(g.mejoraGeneracional || 0, 2)}`);
  setText("gaTemperatura", formato(g.temperaturaAnnealing || 0, 2));
  setText("gaInyecciones", `${numero.format(g.inyeccionesDiferenciales || 0)} DE`);
  setText("gaMetaheuristicas", numero.format((g.metaheuristicas || []).length || 2));

  setText("gaMejorFitness", formato(g.fitnessDelRepresentantePareto ?? g.mejorFitness, 2));
  setText("gaFitnessPromedio", formato(g.fitnessPromedio, 2));
  actualizarDueloGa(g);
  setText("gaDiversidad", `${formato(g.diversidad * 100, 1)}%`);
  setText("gaTasaMutacion", `${formato(g.tasaMutacion * 100, 1)}%`);
  setText("gaConvergencia", `${formato((1 - g.diversidad) * 100, 1)}%`);

  const pesosContainer = $("gaPesos");
  if (!pesosContainer || !g.mejoresPesos) return;

  const labels = ["Utilidad", "Frescura", "Liquidez", "Confiab.", "Z-Score"];
  const maxPeso = Math.max(...g.mejoresPesos, 0.01);

  pesosContainer.textContent = "";
  g.mejoresPesos.forEach((peso, i) => {
    const div = document.createElement("div");
    div.className = "peso-bar";
    const pct = (peso / maxPeso) * 100;
    div.innerHTML = `
      <span class="peso-label">${labels[i]}</span>
      <div class="peso-track">
        <div class="peso-bar-fill" style="width:${pct}%"></div>
      </div>
      <strong>${formato(peso * 100, 0)}%</strong>
    `;
    pesosContainer.appendChild(div);
  });

  registrarPulsoGa(g);
  dibujarGa(g);
}

function actualizarDueloGa(g) {
  const el = $("gaDuelo");
  if (!el) return;
  const representante = Number(g.fitnessDelRepresentantePareto ?? g.mejorFitness ?? 0);
  const retador = Number(g.retadorFitness ?? g.fitnessPromedio ?? 0);
  const baseline = Number(g.baselineFitness ?? retador);
  const gaSuperaBaseline = representante > baseline;
  const campeon = gaSuperaBaseline ? representante : baseline;
  const delta = representante - baseline;
  const empateTecnico = Math.abs(delta) < 0.005;

  const validacion = g.validacion || {};
  const campeonNombre = validacion.campeon || "ga_offline_v1";
  if (empateTecnico) {
    el.textContent = `${campeonNombre} retenido · empate técnico ${formato(campeon, 2)}`;
    el.title = "El campeón offline permanece inmutable; el GA live sólo explora.";
  } else if (delta > 0) {
    el.textContent = `Explorer live +${formato(delta, 2)} · sin autoridad de promoción`;
    el.title = "Superar fitness live no cambia ejecución; hace falta un nuevo artefacto offline revisado.";
  } else {
    el.textContent = `${campeonNombre} retenido · explorer ${formato(Math.abs(delta), 2)} por debajo`;
    el.title = "El motor conserva el campeón offline validado.";
  }

  const evidencia = $("gaValidacion");
  if (evidencia) {
    const sha = typeof validacion.datasetHash === "string" && validacion.datasetHash.length > 12
      ? `${validacion.datasetHash.slice(0, 12)}…`
      : validacion.datasetHash || "sin huella";
    evidencia.textContent = `${validacion.campeon || "baseline"} retenido · ${validacion.holdoutSellado ? "holdout sellado" : "holdout abierto"} · ${validacion.semillasEntrenamiento || 0}/${validacion.semillasHoldout || 0} semillas · SHA ${sha}`;
  }
}

function renderMlEdge(ml) {
  setText("mlEv", dinero.format(ml?.expectedValueUsd || 0));
  setText("mlConfianza", `${formato((ml?.confianza || 0) * 100, 1)}%`);
  setText("mlSurvival", `${formato((ml?.survivalProbability || 0) * 100, 1)}%`);
  setText("mlFill", `${formato((ml?.fillProbability || 0) * 100, 1)}%`);
  const expEl = $("mlExplicacion");
  if (expEl) {
    if (ml?.explicacion) {
      const parts = ml.explicacion.split("; ");
      const main = parts[0] || "";
      const dec = parts[1] || "";

      const metrics = main.split(": ");
      const title = metrics[0] || "";
      const statsStr = metrics[1] || "";
      const statsArr = statsStr.split(/, | y /).filter(s => s.trim() !== "");

      expEl.innerHTML = `
        <div class="ml-explicacion-bloque">
          <strong>${title}</strong>
          <ul>
            ${statsArr.map(s => `<li>${s}</li>`).join("")}
          </ul>
          ${dec ? `<p>${dec}</p>` : ""}
        </div>
      `;
    } else {
      expEl.textContent = "Ejecuta un escenario o selecciona una ruta para calcular el score evolutivo.";
    }
  }
  const container = $("mlFeatures");
  if (!container) return;
  container.textContent = "";
  const variables = ml?.features || [];
  if (variables.length === 0) {
    const vacio = document.createElement("p");
    vacio.className = "mini-empty";
    vacio.textContent = "Sin variables calculadas todavía.";
    container.appendChild(vacio);
    return;
  }

  const max = Math.max(...variables.map((f) => Math.abs(f.contribucion || 0)), 0.01);
  variables.forEach((variable) => {
    const row = document.createElement("div");
    row.className = "ml-feature";
    const ancho = Math.min(100, (Math.abs(variable.contribucion || 0) / max) * 100);
    row.innerHTML = `
      <div>
        <strong>${escapeHtml(nombreFeature(variable.nombre))}</strong>
        <span>peso ${formato((variable.peso || 0) * 100, 0)}% · valor ${formato((variable.valor || 0) * 100, 0)}%</span>
      </div>
      <div class="ml-feature-track"><div style="width:${ancho}%"></div></div>
      <em>${formato(variable.contribucion || 0, 3)}</em>
    `;
    container.appendChild(row);
  });
}

function registrarPulsoGa(g) {
  const ultimo = gaHistorial[gaHistorial.length - 1];
  if (ultimo && ultimo.generacion === g.generacion && ultimo.mejor === g.mejorFitness) return;
  gaHistorial.push({
    generacion: g.generacion || 0,
    mejor: g.mejorFitness || 0,
    retador: g.retadorFitness ?? g.fitnessPromedio ?? 0,
    promedio: g.fitnessPromedio || 0,
    diversidad: g.diversidad || 0,
    mejora: g.mejoraGeneracional || 0,
  });
  if (gaHistorial.length > 96) gaHistorial.shift();
}

function renderResumenLlm(datos) {
  const el = $("resumenLlm");
  if (!el) return;
  const oportunidades = oportunidadesVigentes(datos);
  const mejor = [...oportunidades].sort((a, b) => b.diferencialNetoBps - a.diferencialNetoBps)[0];
  const ejecutable = oportunidades.find((o) => o.ejecutable);
  const g = datos.genetico;
  const esDemoRentable = (datos.eventosEjecucion || []).some((e) => String(e.tipo || "") === "demo_rentable");
  const modo = esDemoRentable
    ? "Escenario rentable: PnL positivo generado para revisar el flujo completo."
    : "Sesión de mercado: esperando una ruta viable o una prueba controlada.";
  const ruta = mejor
    ? `${mejor.compraEn} -> ${mejor.ventaEn} · ${formato(mejor.diferencialNetoBps, 2)} bps netos${mejor.ejecutable ? " · candidata" : " · descartada"}`
    : "sin rutas suficientes";
  const accion = ejecutable
    ? `ruta viva aceptable ${ejecutable.compraEn} -> ${ejecutable.ventaEn} por ${dinero.format(ejecutable.utilidadUsd)}`
    : "sin ruta viva que supere costos y riesgo en este ciclo";
  const ga = g
    ? `GA gen ${g.generacion}, fitness ${formato(g.mejorFitness, 2)}, diversidad ${formato(g.diversidad * 100, 1)}%, umbral ${formato(g.umbralOptimizado, 2)} bps`
    : "GA sin estado";
  const ml = datos.mlEdge
    ? `EV ${dinero.format(datos.mlEdge.expectedValueUsd || 0)}, confianza ${formato((datos.mlEdge.confianza || 0) * 100, 1)}%, decisión ${datos.mlEdge.decision || "sin código"}.`
    : "Esperando decisión auditada.";
  const persistencia = datos.persistencia?.activa
    ? `Auditoría ${datos.persistencia.backend || "durable"} (${datos.persistencia.storageStatus || "estado desconocido"}): ${numero.format(datos.persistencia.operaciones || 0)} trades, ${numero.format(datos.persistencia.oportunidades || 0)} oportunidades y ${numero.format(datos.persistencia.auditorias || 0)} decisiones; cola con ${numero.format(datos.persistencia.queuePending || 0)} pendientes, ${numero.format(datos.persistencia.queueDropped || 0)} descartadas y ${numero.format(datos.persistencia.queueFailed || 0)} fallidas.`
    : "Auditoría durable no disponible.";
  const modoEl = $("llm-modo"); if (modoEl) modoEl.textContent = modo;
  const pnlEl = $("llm-pnl"); if (pnlEl) pnlEl.textContent = dinero.format(datos.metricas.utilidadAcumuladaUsd);
  const riesgoEl = $("llm-riesgo"); if (riesgoEl) riesgoEl.textContent = datos.metricas.estadoRiesgo;
  const rutaEl = $("llm-ruta"); if (rutaEl) rutaEl.textContent = ruta;
  const decisionEl = $("llm-decision"); if (decisionEl) decisionEl.textContent = accion;
  const gaEl = $("llm-ga"); if (gaEl) gaEl.textContent = ga;
  const mlEl = $("llm-ml"); if (mlEl) mlEl.textContent = ml;
  const sistemaEl = $("llm-sistema"); if (sistemaEl) sistemaEl.textContent = persistencia;
  const latenciaEl = $("llm-latencia"); if (latenciaEl) latenciaEl.textContent = `${formato(datos.metricas.latenciaPromedioMs, 0)} ms · ${numero.format(datos.metricas.eventosMercado)} eventos`;
}

function renderExchanges(datos) {
  const container = $("exchangeToggles");
  if (!container) return;

  container.textContent = "";

  const activos = datos.exchangesActivos || {};
  const desdeConfig = Object.keys(datos.configuracion?.exchanges || {});
  const desdeCotizaciones = (datos.cotizaciones || []).map(c => c.exchange);
  const exts = [...new Set([...desdeConfig, ...desdeCotizaciones])].sort();

  exts.forEach(nombre => {
    const div = document.createElement("div");
    div.className = "toggle-exc";
    const label = document.createElement("label");
    label.textContent = nombre;
    const btn = document.createElement("button");
    const estaActivo = activos[nombre] !== false;
    btn.className = `switch-btn ${estaActivo ? "activo" : "inactivo"}`;
    btn.textContent = estaActivo ? "Activo" : "Inactivo";
    btn.dataset.exchange = nombre;
    btn.onclick = async () => {
      const nuevoEstado = btn.classList.contains("activo") ? false : true;
      try {
        const res = await fetch("/api/exchanges", {
          method: "POST",
          headers: headersMutacion({ "Content-Type": "application/json" }),
          body: JSON.stringify({ exchange: nombre, activo: nuevoEstado }),
        });
        if (res.ok) {
          btn.className = `switch-btn ${nuevoEstado ? "activo" : "inactivo"}`;
          btn.textContent = nuevoEstado ? "Activo" : "Inactivo";
          if (ultimoEstado?.exchangesActivos) {
            ultimoEstado.exchangesActivos[nombre] = nuevoEstado;
          }
        }
      } catch (e) {
        debugError("Error toggling exchange", e);
      }
    };
    div.appendChild(label);
    div.appendChild(btn);
    container.appendChild(div);
  });
}

function setText(id, val) {
  const el = $(id);
  if (el) el.textContent = val;
}

function escapeHtml(valor) {
  return String(valor ?? "")
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

function detectarNotificaciones(datos) {
  oportunidadesVigentes(datos).forEach((o) => {
    if (o.ejecutable && o.utilidadUsd > 50 && !opsNotificadas.has(o.id)) {
      opsNotificadas.add(o.id);
      if (opsNotificadas.size > 200) {
        const iter = opsNotificadas.values();
        for (let i = 0; i < 50; i++) {
          opsNotificadas.delete(iter.next().value);
        }
      }
      lanzarNotificacion(o);
    }
  });
}

function lanzarNotificacion(o) {
  window.dispatchEvent(new CustomEvent("mayab:arbitraje", { detail: o }));
  const container = $("notificaciones");
  if (!container) return;

  const div = document.createElement("div");
  div.className = "notificacion";

  const title = document.createElement("strong");
  title.style.color = "var(--verde)";
  title.textContent = "Oportunidad aceptable";

  const route = document.createElement("span");
  route.innerHTML = `Comprar en <strong>${escapeHtml(o.compraEn)}</strong> y vender en <strong>${escapeHtml(o.ventaEn)}</strong>`;

  const profit = document.createElement("span");
  profit.innerHTML = `Utilidad neta estimada: <strong>${dinero.format(o.utilidadUsd)}</strong> (${formato(o.diferencialNetoBps, 2)} bps)`;

  const closeBtn = document.createElement("button");
  closeBtn.className = "btn-cerrar-notif";
  closeBtn.innerHTML = "&times;";
  closeBtn.onclick = () => {
    div.style.animation = "slideOutRight 0.3s cubic-bezier(0.16, 1, 0.3, 1) forwards";
    setTimeout(() => { if (div.parentNode) div.remove(); }, 300);
  };

  div.appendChild(closeBtn);
  div.appendChild(title);
  div.appendChild(route);
  div.appendChild(profit);
  container.appendChild(div);
  while (container.children.length > 3) {
    container.firstElementChild?.remove();
  }

  setTimeout(() => {
    if (div.parentNode) {
      div.style.animation = "slideOutRight 0.3s cubic-bezier(0.16, 1, 0.3, 1) forwards";
      setTimeout(() => {
        if (div.parentNode) div.remove();
      }, 300);
    }
  }, 8000);
}

function iniciarVisualizacionesLive() {
  let framePendiente = false;
  const redibujar = () => {
    if (framePendiente) return;
    framePendiente = true;
    requestAnimationFrame(() => {
      framePendiente = false;
      if (ultimoEstado) dibujarPnlLive(ultimoEstado);
    });
  };
  window.addEventListener("resize", redibujar, { passive: true });
  window.addEventListener("mayab:tab-visible", redibujar);
}

function renderHeatmapOportunidades(datos) {
  const table = $("heatmapOportunidades");
  const head = $("heatmapHead");
  const body = $("heatmapBody");
  const empty = $("heatmapEmpty");
  const status = $("heatmapStatus");
  const summary = $("heatmapSummary");
  const scroll = table?.closest(".heatmap-scroll");
  if (!table || !head || !body || !empty || !scroll) return;

  const oportunidades = oportunidadesVigentes(datos);
  const cotizaciones = (datos?.cotizaciones || []).filter(
    (cotizacion) => parActivo === "ALL" || cotizacion.par === parActivo,
  );
  const exchanges = [...new Set([
    ...cotizaciones.map((cotizacion) => cotizacion.exchange),
    ...oportunidades.flatMap((oportunidad) => [oportunidad.compraEn, oportunidad.ventaEn]),
  ].filter(Boolean))].sort((a, b) => a.localeCompare(b, "es-MX"));

  head.replaceChildren();
  body.replaceChildren();
  const matrizDisponible = exchanges.length >= 2;
  scroll.hidden = !matrizDisponible;
  empty.hidden = matrizDisponible;

  const etiquetaPar = parActivo === "ALL" ? "todos los pares" : parActivo;
  if (!matrizDisponible) {
    empty.textContent = `Se necesitan al menos dos exchanges con datos para ${etiquetaPar}.`;
    if (status) status.textContent = "Sin matriz disponible";
    const texto = `Heatmap sin datos: hay ${exchanges.length} exchange con cotización para ${etiquetaPar}.`;
    if (summary && summary.textContent !== texto) summary.textContent = texto;
    return;
  }

  const rutas = new Map();
  oportunidades.forEach((oportunidad) => {
    const valor = Number(oportunidad.diferencialNetoBps);
    if (!Number.isFinite(valor) || !oportunidad.compraEn || !oportunidad.ventaEn) return;
    const clave = `${oportunidad.compraEn}\u0000${oportunidad.ventaEn}`;
    const actual = rutas.get(clave);
    if (!actual) {
      rutas.set(clave, {
        mejor: oportunidad,
        valor,
        cantidad: 1,
        ejecutables: oportunidad.ejecutable ? 1 : 0,
      });
      return;
    }
    actual.cantidad += 1;
    if (oportunidad.ejecutable) actual.ejecutables += 1;
    if (valor > actual.valor) {
      actual.mejor = oportunidad;
      actual.valor = valor;
    }
  });

  const maxAbs = Math.max(1, ...[...rutas.values()].map((ruta) => Math.abs(ruta.valor)));
  const headerRow = document.createElement("tr");
  const corner = document.createElement("th");
  corner.scope = "col";
  corner.textContent = "Compra ↓ / Venta →";
  headerRow.appendChild(corner);
  exchanges.forEach((exchange) => {
    const th = document.createElement("th");
    th.scope = "col";
    th.textContent = exchange;
    headerRow.appendChild(th);
  });
  head.appendChild(headerRow);

  exchanges.forEach((compra) => {
    const row = document.createElement("tr");
    const rowHeader = document.createElement("th");
    rowHeader.scope = "row";
    rowHeader.textContent = compra;
    row.appendChild(rowHeader);

    exchanges.forEach((venta) => {
      const td = document.createElement("td");
      td.className = "heatmap-cell";
      if (compra === venta) {
        td.classList.add("heat-diagonal");
        td.textContent = "—";
        td.setAttribute("aria-label", `${compra}: compra y venta en el mismo exchange no aplica.`);
        row.appendChild(td);
        return;
      }

      const ruta = rutas.get(`${compra}\u0000${venta}`);
      if (!ruta) {
        td.classList.add("heat-empty-cell");
        td.textContent = "·";
        td.setAttribute("aria-label", `Sin ruta vigente para comprar en ${compra} y vender en ${venta}.`);
        row.appendChild(td);
        return;
      }

      const intensidad = Math.min(1, Math.abs(ruta.valor) / maxAbs);
      const mezcla = Math.round(20 + intensidad * 66);
      td.style.setProperty("--heat-mix", `${mezcla}%`);
      td.classList.add(ruta.valor >= 0 ? "heat-positive-cell" : "heat-negative-cell");
      if (intensidad >= 0.68) td.classList.add("heat-strong");
      if (ruta.ejecutables > 0) td.classList.add("heat-executable");

      const value = document.createElement("span");
      value.className = "heatmap-value";
      value.textContent = `${ruta.valor > 0 ? "+" : ""}${formato(ruta.valor, 2)} bps`;
      const count = document.createElement("small");
      count.className = "heatmap-count";
      count.textContent = ruta.ejecutables > 0
        ? `${ruta.cantidad} ruta${ruta.cantidad === 1 ? "" : "s"} · ejecutable`
        : `${ruta.cantidad} ruta${ruta.cantidad === 1 ? "" : "s"}`;
      td.append(value, count);
      const ejecucion = ruta.ejecutables > 0 ? " Hay al menos una ruta ejecutable." : " Ninguna es ejecutable.";
      td.setAttribute(
        "aria-label",
        `Comprar en ${compra} y vender en ${venta}: mejor diferencial neto ${value.textContent} en ${ruta.cantidad} ruta${ruta.cantidad === 1 ? "" : "s"}.${ejecucion}`,
      );
      td.title = `${compra} → ${venta} · ${value.textContent} · ${ruta.mejor.par || etiquetaPar}`;
      row.appendChild(td);
    });
    body.appendChild(row);
  });

  const ejecutables = oportunidades.filter((oportunidad) => oportunidad.ejecutable).length;
  if (status) status.textContent = `${oportunidades.length} rutas · ${exchanges.length} exchanges`;
  const mejor = [...rutas.values()].sort((a, b) => b.valor - a.valor)[0];
  const textoResumen = mejor
    ? `Heatmap de ${oportunidades.length} rutas vigentes para ${etiquetaPar} entre ${exchanges.length} exchanges. ${ejecutables} son ejecutables. La mejor compra es ${mejor.mejor.compraEn} y la mejor venta es ${mejor.mejor.ventaEn}, con ${formato(mejor.valor, 2)} puntos base netos.`
    : `Heatmap para ${etiquetaPar} entre ${exchanges.length} exchanges, sin rutas vigentes en el último lote.`;
  if (summary && summary.textContent !== textoResumen) summary.textContent = textoResumen;
}

function dibujarPnlLive(datos) {
  const canvas = $("canvasPnlLive");
  if (!canvas) return;

  const puntos = normalizarSerieTemporal(datos?.seriePnl);
  const valorMetrica = Number(datos?.metricas?.utilidadAcumuladaUsd);
  const valorActual = Number.isFinite(valorMetrica) ? valorMetrica : (puntos.at(-1)?.valor || 0);
  const primerValor = puntos[0]?.valor ?? valorActual;
  const delta = valorActual - primerValor;
  setText("pnlLiveValue", dinero.format(valorActual));
  setText("pnlLiveDelta", `${delta > 0 ? "+" : ""}${dinero.format(delta)}`);
  setText("pnlLiveSamples", numero.format(puntos.length));

  const deltaEl = $("pnlLiveDelta");
  if (deltaEl) {
    deltaEl.classList.toggle("positivo", delta > 0);
    deltaEl.classList.toggle("negativo", delta < 0);
  }
  const status = $("pnlLiveStatus");
  if (status) status.textContent = puntos.length
    ? `WS · ${puntos.length} muestra${puntos.length === 1 ? "" : "s"}`
    : "Esperando historial";

  const summary = $("pnlLiveSummary");
  const minimo = puntos.length ? Math.min(...puntos.map((punto) => punto.valor)) : valorActual;
  const maximo = puntos.length ? Math.max(...puntos.map((punto) => punto.valor)) : valorActual;
  const textoResumen = puntos.length
    ? `PnL acumulado actual ${dinero.format(valorActual)}. Cambio en la ventana visible ${dinero.format(delta)}. Mínimo ${dinero.format(minimo)}, máximo ${dinero.format(maximo)}, con ${puntos.length} muestras entre ${formatoHoraGrafica.format(new Date(puntos[0].tiempo))} y ${formatoHoraGrafica.format(new Date(puntos.at(-1).tiempo))}.`
    : `PnL acumulado actual ${dinero.format(valorActual)}. El motor todavía no ha publicado puntos en la serie temporal.`;
  if (summary && summary.textContent !== textoResumen) summary.textContent = textoResumen;

  if (canvas.getBoundingClientRect().width < 2) return;
  const ctx = prepararCanvas(canvas);
  const w = canvas._anchoLogico;
  const h = canvas._altoLogico;
  const temaOscuro = document.documentElement.getAttribute("data-theme") === "dark";
  const fondo = temaOscuro ? "#171717" : "#f9f7f2";
  const tinta = temaOscuro ? "#f4f0e6" : "#141414";
  const muted = temaOscuro ? "#a7a096" : "#6b625b";
  const grid = temaOscuro ? "rgba(244,240,230,0.12)" : "rgba(20,20,20,0.12)";
  const positivo = temaOscuro ? "#7ee787" : "#0b6b35";
  const negativo = temaOscuro ? "#ff8a70" : "#c83b20";
  const colorLinea = valorActual >= 0 ? positivo : negativo;

  ctx.clearRect(0, 0, w, h);
  ctx.fillStyle = fondo;
  ctx.fillRect(0, 0, w, h);

  const margen = {
    top: 22,
    right: w < 480 ? 12 : 22,
    bottom: 38,
    left: w < 480 ? 54 : 66,
  };
  const plotW = Math.max(1, w - margen.left - margen.right);
  const plotH = Math.max(1, h - margen.top - margen.bottom);

  if (!puntos.length) {
    ctx.strokeStyle = grid;
    ctx.lineWidth = 1;
    for (let i = 0; i <= 4; i += 1) {
      const y = margen.top + (i / 4) * plotH;
      ctx.beginPath();
      ctx.moveTo(margen.left, y);
      ctx.lineTo(w - margen.right, y);
      ctx.stroke();
    }
    ctx.fillStyle = tinta;
    ctx.font = "800 15px Aeonik Pro, sans-serif";
    ctx.textAlign = "center";
    ctx.fillText("Esperando historial de PnL", margen.left + plotW / 2, margen.top + plotH / 2 - 4, plotW - 12);
    ctx.fillStyle = muted;
    ctx.font = "700 12px Aeonik Pro, sans-serif";
    ctx.fillText("Aparece con la primera operación simulada", margen.left + plotW / 2, margen.top + plotH / 2 + 18, plotW - 12);
    return;
  }

  const tiempoPrimero = puntos[0].tiempo;
  const tiempoUltimo = puntos.at(-1).tiempo;
  const tiempoMin = puntos.length === 1 ? tiempoPrimero - 30_000 : tiempoPrimero;
  const tiempoMax = puntos.length === 1 ? tiempoUltimo + 30_000 : Math.max(tiempoUltimo, tiempoPrimero + 1);
  const brutoMin = Math.min(0, minimo);
  const brutoMax = Math.max(0, maximo);
  const amplitudBruta = brutoMax - brutoMin;
  const paddingY = Math.max(amplitudBruta * 0.1, Math.max(Math.abs(brutoMin), Math.abs(brutoMax)) * 0.04, 0.5);
  let yMin = brutoMin < 0 ? brutoMin - paddingY : 0;
  let yMax = brutoMax > 0 ? brutoMax + paddingY : 0;
  if (yMax - yMin < 1) {
    yMin = -1;
    yMax = 1;
  }

  const x = (tiempo) => margen.left + ((tiempo - tiempoMin) / (tiempoMax - tiempoMin)) * plotW;
  const y = (valor) => margen.top + ((yMax - valor) / (yMax - yMin)) * plotH;

  ctx.font = "700 11px Aeonik Pro, sans-serif";
  ctx.lineWidth = 1;
  for (let i = 0; i <= 4; i += 1) {
    const proporcion = i / 4;
    const valorTick = yMax - proporcion * (yMax - yMin);
    const yTick = margen.top + proporcion * plotH;
    ctx.strokeStyle = grid;
    ctx.beginPath();
    ctx.moveTo(margen.left, yTick);
    ctx.lineTo(w - margen.right, yTick);
    ctx.stroke();
    ctx.fillStyle = muted;
    ctx.textAlign = "right";
    ctx.fillText(formatoEjeUsd(valorTick), margen.left - 9, yTick + 4);
  }

  [0, 0.5, 1].forEach((proporcion, indice) => {
    const tiempo = tiempoMin + proporcion * (tiempoMax - tiempoMin);
    const xTick = margen.left + proporcion * plotW;
    ctx.strokeStyle = grid;
    ctx.beginPath();
    ctx.moveTo(xTick, margen.top);
    ctx.lineTo(xTick, margen.top + plotH);
    ctx.stroke();
    ctx.fillStyle = muted;
    ctx.textAlign = indice === 0 ? "left" : indice === 2 ? "right" : "center";
    ctx.fillText(formatoHoraGrafica.format(new Date(tiempo)), xTick, h - 13);
  });

  const ceroY = y(0);
  ctx.save();
  ctx.setLineDash([6, 5]);
  ctx.strokeStyle = temaOscuro ? "rgba(244,240,230,0.55)" : "rgba(20,20,20,0.5)";
  ctx.lineWidth = 1.25;
  ctx.beginPath();
  ctx.moveTo(margen.left, ceroY);
  ctx.lineTo(w - margen.right, ceroY);
  ctx.stroke();
  ctx.restore();

  ctx.save();
  ctx.beginPath();
  ctx.rect(margen.left, margen.top, plotW, plotH);
  ctx.clip();
  const gradiente = ctx.createLinearGradient(0, margen.top, 0, margen.top + plotH);
  gradiente.addColorStop(0, `${colorLinea}5c`);
  gradiente.addColorStop(1, `${colorLinea}05`);
  ctx.fillStyle = gradiente;
  ctx.beginPath();
  ctx.moveTo(x(puntos[0].tiempo), ceroY);
  puntos.forEach((punto) => ctx.lineTo(x(punto.tiempo), y(punto.valor)));
  ctx.lineTo(x(puntos.at(-1).tiempo), ceroY);
  ctx.closePath();
  ctx.fill();

  ctx.strokeStyle = colorLinea;
  ctx.lineWidth = 3.5;
  ctx.lineJoin = "round";
  ctx.lineCap = "round";
  ctx.beginPath();
  puntos.forEach((punto, indice) => {
    const px = x(punto.tiempo);
    const py = y(punto.valor);
    if (indice === 0) ctx.moveTo(px, py);
    else ctx.lineTo(px, py);
  });
  ctx.stroke();
  ctx.restore();

  const ultimo = puntos.at(-1);
  const ultimoX = x(ultimo.tiempo);
  const ultimoY = y(ultimo.valor);
  ctx.fillStyle = colorLinea;
  ctx.beginPath();
  ctx.arc(ultimoX, ultimoY, 5, 0, Math.PI * 2);
  ctx.fill();
  ctx.strokeStyle = colorLinea;
  ctx.globalAlpha = 0.35;
  ctx.lineWidth = 2;
  ctx.beginPath();
  ctx.arc(ultimoX, ultimoY, 9, 0, Math.PI * 2);
  ctx.stroke();
  ctx.globalAlpha = 1;
}

function formatoEjeUsd(valor) {
  const absoluto = Math.abs(valor);
  if (absoluto >= 1_000_000) return `${valor < 0 ? "−" : ""}$${formato(absoluto / 1_000_000, 1)}M`;
  if (absoluto >= 1_000) return `${valor < 0 ? "−" : ""}$${formato(absoluto / 1_000, 1)}k`;
  return `${valor < 0 ? "−" : ""}$${formato(absoluto, absoluto < 10 ? 1 : 0)}`;
}

/**
 * @description Dibuja el mapa de rutas de arbitraje en un canvas 2D usando curvas Bézier.
 * Las rutas ejecutables se muestran en verde y las descartadas en rojo oscuro.
 * @param {Object} datos - El estado público actual.
 */
function dibujarMapa(datos) {
  const canvas = $("canvasMapa");
  if (!canvas) return;
  const ctx = prepararCanvas(canvas);
  const w = canvas._anchoLogico;
  const h = canvas._altoLogico;
  ctx.clearRect(0, 0, w, h);

  const temaOscuro = document.documentElement.getAttribute("data-theme") === "dark";
  const colorTinta = temaOscuro ? "#f4f0e6" : "#141414";
  const colorFondo = temaOscuro ? "#111111" : "#f2efe9";
  const colorMuted = temaOscuro ? "#888888" : "#666666";

  fondoArquitectonico(ctx, w, h, temaOscuro);

  const exchanges = datos.cotizaciones.map((c) => c.exchange);
  const centroX = w * 0.5;
  const centroY = h * 0.52;
  const radioX = w * 0.34;
  const radioY = h * 0.32;
  const posiciones = new Map();

  exchanges.forEach((nombre, i) => {
    const angulo = -Math.PI / 2 + (Math.PI * 2 * i) / Math.max(exchanges.length, 1);
    posiciones.set(nombre, {
      x: centroX + Math.cos(angulo) * radioX,
      y: centroY + Math.sin(angulo) * radioY,
    });
  });

  oportunidadesVigentes(datos).slice(0, 18).forEach((o, i) => {
    const a = posiciones.get(o.compraEn);
    const b = posiciones.get(o.ventaEn);
    if (!a || !b) return;
    const fuerza = Math.max(0.18, Math.min(1, o.diferencialNetoBps / 8));
    ctx.strokeStyle = o.ejecutable ? `rgba(38,208,124,${0.32 + fuerza * 0.55})` : `rgba(255,91,63,${0.18 + fuerza * 0.3})`;
    ctx.lineWidth = o.ejecutable ? 2.4 + fuerza * 5 : 1.2;
    ctx.beginPath();
    const dx = b.x - a.x;
    const dy = b.y - a.y;
    ctx.moveTo(a.x, a.y);
    ctx.bezierCurveTo(a.x + dy * 0.14, a.y - dx * 0.14, b.x + dy * 0.14, b.y - dx * 0.14, b.x, b.y);
    ctx.stroke();

    if (i < 5 && o.ejecutable) {
      ctx.fillStyle = "#dfff43";
      const t = (Date.now() / 900 + i * 0.18) % 1;

      const cp1x = a.x + dy * 0.14;
      const cp1y = a.y - dx * 0.14;
      const cp2x = b.x + dy * 0.14;
      const cp2y = b.y - dx * 0.14;

      const mt = 1 - t;
      const mt2 = mt * mt;
      const mt3 = mt2 * mt;
      const t2 = t * t;
      const t3 = t2 * t;

      const x = mt3 * a.x + 3 * mt2 * t * cp1x + 3 * mt * t2 * cp2x + t3 * b.x;
      const y = mt3 * a.y + 3 * mt2 * t * cp1y + 3 * mt * t2 * cp2y + t3 * b.y;

      ctx.beginPath();
      ctx.shadowColor = "#dfff43";
      ctx.shadowBlur = 6;
      ctx.arc(x, y, 4.5, 0, Math.PI * 2);
      ctx.fill();
      ctx.shadowBlur = 0;
    }
  });

  datos.cotizaciones.forEach((c) => {
    const p = posiciones.get(c.exchange);
    if (!p) return;
    ctx.fillStyle = colorFondo;
    ctx.strokeStyle = colorTinta;
    ctx.lineWidth = 2;
    ctx.beginPath();
    ctx.rect(p.x - 56, p.y - 30, 112, 60);
    ctx.fill();
    ctx.stroke();
    ctx.fillStyle = colorTinta;
    ctx.font = "700 17px Archivo, sans-serif";
    ctx.textAlign = "center";
    ctx.fillText(c.exchange, p.x, p.y - 4);
    ctx.fillStyle = colorMuted;
    ctx.font = "700 12px Archivo, sans-serif";
    ctx.fillText(`${formato(c.ask - c.bid, 2)} dif.`, p.x, p.y + 17);
  });
}

/**
 * @description Dibuja la serie temporal de PnL (Ganancias/Pérdidas) acumuladas y spread
 * diferencial en el canvas correspondiente.
 * @param {Object} datos - El estado público actual.
 */
function dibujarSeries(datos) {
  const canvas = $("canvasSeries");
  if (!canvas) return;
  const ctx = prepararCanvas(canvas);
  const w = canvas._anchoLogico;
  const h = canvas._altoLogico;
  ctx.clearRect(0, 0, w, h);

  const temaOscuro = document.documentElement.getAttribute("data-theme") === "dark";
  const pnlColor = temaOscuro ? "#b7ff3c" : "#168a3a";
  const difColor = temaOscuro ? "#a78bfa" : "#6d28d9";

  fondoArquitectonico(ctx, w, h, temaOscuro);

  const seriePnl = normalizarSerieTemporal(datos.seriePnl);
  const serieDif = normalizarSerieTemporal(datos.serieDiferencial);
  const tiempos = [...seriePnl, ...serieDif].map((p) => p.tiempo);
  const tiempoMin = Math.min(...tiempos);
  const tiempoMax = Math.max(...tiempos);
  const dominioTiempo = Number.isFinite(tiempoMin) && Number.isFinite(tiempoMax)
    ? [tiempoMin, Math.max(tiempoMax, tiempoMin + 1)]
    : null;

  dibujarLineaTemporal(ctx, seriePnl, pnlColor, w, h, 0.58, dominioTiempo);
  dibujarLineaTemporal(ctx, serieDif, difColor, w, h, 0.34, dominioTiempo);
  dibujarEjeTemporal(ctx, dominioTiempo, w, h, temaOscuro);

  // Agregar ejes y etiquetas
  ctx.fillStyle = pnlColor;
  ctx.font = "800 14px Archivo, sans-serif";
  ctx.textAlign = "left";
  ctx.fillText(`Utilidad neta acumulada · ${dinero.format(seriePnl.at(-1)?.valor || 0)}`, 24, 30);

  ctx.fillStyle = difColor;
  ctx.fillText(`Diferencial neto · ${formato(serieDif.at(-1)?.valor || 0, 2)} bps`, 24, 52);

  ctx.fillStyle = temaOscuro ? "#a7a096" : "#6b625b";
  ctx.font = "700 11px Aeonik Pro, sans-serif";
  ctx.textAlign = "right";
  ctx.fillText(`PnL ${rangoSerie(seriePnl.map((p) => p.valor), dinero.format)}`, w - 24, 30);
  ctx.fillText(`Spread ${rangoSerie(serieDif.map((p) => p.valor), (v) => `${formato(v, 2)} bps`)}`, w - 24, 52);
}

function dibujarGa(g) {
  const canvas = $("canvasGa");
  if (!canvas) return;
  const ctx = prepararCanvas(canvas);
  const w = canvas._anchoLogico;
  const h = canvas._altoLogico;
  ctx.clearRect(0, 0, w, h);

  const temaOscuro = document.documentElement.getAttribute("data-theme") === "dark";
  ctx.fillStyle = temaOscuro ? "#171717" : "#f9f7f2";
  ctx.fillRect(0, 0, w, h);

  ctx.strokeStyle = temaOscuro ? "#333333" : "#dcd7cc";
  ctx.lineWidth = 1;
  for (let x = 40; x < w; x += 60) {
    ctx.beginPath();
    ctx.moveTo(x, 0);
    ctx.lineTo(x, h);
    ctx.stroke();
  }
  for (let y = 30; y < h; y += 40) {
    ctx.beginPath();
    ctx.moveTo(0, y);
    ctx.lineTo(w, y);
    ctx.stroke();
  }

  const frontera = g?.fronteraPareto || [];
  if (frontera.length === 0) {
    ctx.fillStyle = temaOscuro ? "#f4f0e6" : "#141414";
    ctx.font = "900 13px Inter, sans-serif";
    ctx.textAlign = "center";
    ctx.fillText("Preparando evaluación de Pareto…", w / 2, h / 2);
    return;
  }

  const xs = frontera.map(p => p.x);
  const ys = frontera.map(p => p.y);

  const minX = Math.min(...xs, 0);
  const maxX = Math.max(...xs, 1);
  const minY = Math.min(...ys, 0);
  const maxY = Math.max(...ys, 1);

  const rangeX = Math.max(maxX - minX, 0.1);
  const rangeY = Math.max(maxY - minY, 0.1);

  const px = (val) => 40 + ((val - minX) / rangeX) * (w - 80);
  const py = (val) => h - 40 - ((val - minY) / rangeY) * (h - 80);

  frontera.forEach((p) => {
    const x = px(p.x);
    const y = py(p.y);

    ctx.beginPath();
    ctx.arc(x, y, 6, 0, Math.PI * 2);
    ctx.fillStyle = temaOscuro ? "rgba(32, 230, 154, 0.8)" : "rgba(10, 180, 100, 0.8)";
    ctx.fill();
    ctx.lineWidth = 2;
    ctx.strokeStyle = temaOscuro ? "#111" : "#fff";
    ctx.stroke();
  });

  ctx.fillStyle = temaOscuro ? "#f4f0e6" : "#141414";
  ctx.font = "900 13px Inter, sans-serif";
  ctx.textAlign = "left";
  ctx.fillText(`Frontera de Pareto (NSGA-II) · Sharpe (X) vs Utilidad Media (Y) · ${frontera.length} Puntos`, 20, 22);

  ctx.font = "400 11px Inter, sans-serif";
  ctx.fillText(`Min Sharpe: ${formato(minX, 2)}`, 40, h - 10);
  ctx.textAlign = "right";
  ctx.fillText(`Max Sharpe: ${formato(maxX, 2)}`, w - 20, h - 10);

  ctx.textAlign = "left";
  ctx.fillText(`Max PnL: ${formato(maxY, 2)}`, 10, 40);
  ctx.fillText(`Min PnL: ${formato(minY, 2)}`, 10, h - 30);
}

function trazarSerieGa(ctx, valores, px, py, color, grosor, progreso = 1) {
  ctx.strokeStyle = color;
  ctx.lineWidth = grosor;
  ctx.shadowColor = color;
  ctx.shadowBlur = 7;
  ctx.lineJoin = "round";
  ctx.lineCap = "round";
  ctx.beginPath();
  const ultimoVisible = Math.max(1, Math.ceil((valores.length - 1) * progreso));
  valores.slice(0, ultimoVisible + 1).forEach((valor, i) => {
    const x = px(i);
    const y = py(valor);
    if (i === 0) ctx.moveTo(x, y);
    else ctx.lineTo(x, y);
  });
  ctx.stroke();
  ctx.shadowBlur = 0;
  const x = px(ultimoVisible);
  const y = py(valores[ultimoVisible]);
  ctx.fillStyle = color;
  ctx.beginPath();
  ctx.arc(x, y, 5, 0, Math.PI * 2);
  ctx.fill();
}

function normalizarSerieTemporal(serie = []) {
  return serie
    .map((p) => ({ tiempo: Date.parse(p?.tiempo), valor: Number(p?.valor) }))
    .filter((p) => Number.isFinite(p.tiempo) && Number.isFinite(p.valor))
    .sort((a, b) => a.tiempo - b.tiempo);
}

function dibujarLineaTemporal(ctx, puntos, color, w, h, base, dominioTiempo) {
  if (!puntos.length || !dominioTiempo) return;
  const valores = puntos.map((p) => p.valor);
  const min = Math.min(...valores, 0);
  const max = Math.max(...valores, 1);
  const absMax = Math.max(Math.abs(min), Math.abs(max));
  const max_amp = Math.max(absMax, 1);
  const [tiempoMin, tiempoMax] = dominioTiempo;
  const px = (tiempo) => 28 + ((tiempo - tiempoMin) / (tiempoMax - tiempoMin)) * (w - 56);
  const py = (valor) => h * base - (valor / max_amp) * (h * 0.24);

  ctx.globalAlpha = 0.14;
  ctx.fillStyle = color;
  ctx.beginPath();
  ctx.moveTo(px(puntos[0].tiempo), h * base);
  puntos.forEach((p) => {
    ctx.lineTo(px(p.tiempo), py(p.valor));
  });
  ctx.lineTo(px(puntos.at(-1).tiempo), h * base);
  ctx.closePath();
  ctx.fill();
  ctx.globalAlpha = 1.0;

  ctx.strokeStyle = color;
  ctx.lineWidth = 5.5;
  ctx.lineJoin = "round";
  ctx.lineCap = "round";
  ctx.shadowColor = color;
  ctx.shadowBlur = 9;
  ctx.beginPath();
  puntos.forEach((p, i) => {
    const x = px(p.tiempo);
    const y = py(p.valor);
    if (i === 0) ctx.moveTo(x, y);
    else ctx.lineTo(x, y);
  });
  ctx.stroke();
  ctx.shadowBlur = 0;

  const ultimo = puntos.at(-1);
  ctx.fillStyle = color;
  ctx.strokeStyle = color;
  ctx.lineWidth = 2;
  ctx.beginPath();
  ctx.arc(px(ultimo.tiempo), py(ultimo.valor), 7, 0, Math.PI * 2);
  ctx.fill();
  ctx.globalAlpha = 0.35;
  ctx.beginPath();
  ctx.arc(px(ultimo.tiempo), py(ultimo.valor), 11, 0, Math.PI * 2);
  ctx.stroke();
  ctx.globalAlpha = 1;
}

function dibujarEjeTemporal(ctx, dominioTiempo, w, h, temaOscuro) {
  if (!dominioTiempo) return;
  const [inicio, fin] = dominioTiempo;
  ctx.strokeStyle = temaOscuro ? "#4a4640" : "#d5cec2";
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.moveTo(28, h - 24);
  ctx.lineTo(w - 28, h - 24);
  ctx.stroke();
  ctx.fillStyle = temaOscuro ? "#a7a096" : "#6b625b";
  ctx.font = "700 11px Aeonik Pro, sans-serif";
  [inicio, inicio + (fin - inicio) / 2, fin].forEach((tiempo, i) => {
    ctx.textAlign = i === 0 ? "left" : i === 2 ? "right" : "center";
    ctx.fillText(formatoHoraGrafica.format(new Date(tiempo)), 28 + (i / 2) * (w - 56), h - 7);
  });
}

function easeOutCubic(t) {
  return 1 - Math.pow(1 - t, 3);
}

function rangoSerie(valores, formatear) {
  if (!valores.length) return "sin muestras";
  return `${formatear(Math.min(...valores))} — ${formatear(Math.max(...valores))}`;
}

function fondoArquitectonico(ctx, w, h, temaOscuro) {
  ctx.fillStyle = temaOscuro ? "#171717" : "#f9f7f2";
  ctx.fillRect(0, 0, w, h);

  ctx.strokeStyle = temaOscuro ? "#333333" : "#dcd7cc";
  ctx.lineWidth = 1;
  for (let x = 0; x < w; x += 52) {
    ctx.beginPath();
    ctx.moveTo(x, 0);
    ctx.lineTo(x, h);
    ctx.stroke();
  }
  for (let y = 0; y < h; y += 44) {
    ctx.beginPath();
    ctx.moveTo(0, y);
    ctx.lineTo(w, y);
    ctx.stroke();
  }
}

function prepararCanvas(canvas) {
  const ratio = window.devicePixelRatio || 1;
  const rect = canvas.getBoundingClientRect();
  // El ancho debe seguir al contenedor: forzar 320 px desbordaba los paneles
  // en teléfonos estrechos y terminaba recortando la parte derecha del gráfico.
  const ancho = Math.max(1, Math.floor(rect.width));
  const alto = Math.max(220, Math.floor(rect.height));
  const anchoFisico = Math.floor(ancho * ratio);
  const altoFisico = Math.floor(alto * ratio);
  if (canvas.width !== anchoFisico || canvas.height !== altoFisico) {
    canvas.width = anchoFisico;
    canvas.height = altoFisico;
  }
  const ctx = canvas.getContext("2d");
  ctx.setTransform(ratio, 0, 0, ratio, 0, 0);
  canvas._anchoLogico = ancho;
  canvas._altoLogico = alto;
  return ctx;
}

function actualizarMejorDiferencial(datos) {
  const el = $("mejorDiferencial");
  if (!el) return;
  const oportunidades = oportunidadesVigentes(datos);

  const countEl = $("conteoRutas");
  if (countEl) {
    countEl.textContent = `${oportunidades.length} RUTAS`;
  }

  if (oportunidades.length === 0) {
    el.textContent = "sin rutas";
    el.className = "mapa-bps neutro";
    return;
  }
  const mejor = oportunidades.reduce(
    (acc, o) => Math.max(acc, Number(o.diferencialNetoBps || 0)),
    Number.NEGATIVE_INFINITY,
  );
  el.textContent = `${formato(mejor, 2)} bps`;
  el.className = `mapa-bps ${mejor >= 0 ? "positivo" : "negativo"}`;
}

// El backend conserva una cola corta para auditoría. Para superficies marcadas
// como LIVE mostramos únicamente el lote del análisis más reciente.
function oportunidadesVigentes(datos) {
  const oportunidades = Array.isArray(datos?.oportunidades) ? datos.oportunidades : [];
  if (oportunidades.length < 2) {
    return parActivo === "ALL"
      ? oportunidades
      : oportunidades.filter((oportunidad) => oportunidad.par === parActivo);
  }

  let instanteMasReciente = Number.NEGATIVE_INFINITY;
  for (const oportunidad of oportunidades) {
    const instante = Date.parse(oportunidad.detectadaEn || "");
    if (Number.isFinite(instante)) instanteMasReciente = Math.max(instanteMasReciente, instante);
  }
  if (!Number.isFinite(instanteMasReciente)) {
    return parActivo === "ALL"
      ? oportunidades
      : oportunidades.filter((oportunidad) => oportunidad.par === parActivo);
  }

  const vigentes = oportunidades.filter((oportunidad) => {
    const instante = Date.parse(oportunidad.detectadaEn || "");
    return Number.isFinite(instante) && instante === instanteMasReciente;
  });
  return parActivo === "ALL"
    ? vigentes
    : vigentes.filter((oportunidad) => oportunidad.par === parActivo);
}

function formato(valor, decimales) {
  return Number(valor || 0).toLocaleString("es-MX", {
    minimumFractionDigits: decimales,
    maximumFractionDigits: decimales,
  });
}

// Navegación de pestañas (Tabs)
document.addEventListener("DOMContentLoaded", () => {
  const pantalla = document.querySelector(".pantalla");
  const scrollPorTab = new Map();

  const guardarScroll = () => {
    const tabActivo = document.querySelector(".tab-content.activo")?.id;
    if (!pantalla || !tabActivo) return;
    scrollPorTab.set(tabActivo, pantalla.scrollTop);
  };
  iniciarLanding();
  iniciarDescargaEvidencia();
  iniciarSelectorProcedencia();
  cargarEscalaCorpusPublico();
  iniciarAjusteMetricas();
  iniciarDiccionario();
  iniciarHerramientasTablas();
  iniciarVisualizacionesLive();

  const mobileNavToggle = document.querySelector(".mobile-nav-toggle");
  const tabsNav = document.querySelector(".tabs-nav");
  const mobileNavLabel = document.getElementById("mobileNavLabel");
  const tabButtons = [...document.querySelectorAll(".tab-btn")];
  tabsNav?.setAttribute("role", "tablist");
  tabButtons.forEach((button) => {
    const targetId = button.getAttribute("data-tab");
    const targetContent = targetId ? document.getElementById(targetId) : null;
    if (!targetId || !targetContent) return;
    if (!button.id) button.id = `${targetId}-control`;
    button.setAttribute("role", "tab");
    button.setAttribute("aria-controls", targetId);
    button.setAttribute("aria-selected", String(button.classList.contains("activo")));
    button.tabIndex = button.classList.contains("activo") ? 0 : -1;
    targetContent.setAttribute("role", "tabpanel");
    targetContent.setAttribute("aria-labelledby", button.id);
  });
  const cerrarMenuMobile = () => {
    mobileNavToggle?.setAttribute("aria-expanded", "false");
    tabsNav?.classList.remove("mobile-open");
  };
  mobileNavToggle?.addEventListener("click", () => {
    const abrir = mobileNavToggle.getAttribute("aria-expanded") !== "true";
    mobileNavToggle.setAttribute("aria-expanded", String(abrir));
    tabsNav?.classList.toggle("mobile-open", abrir);
  });
  document.addEventListener("click", (event) => {
    if (!mobileNavToggle?.contains(event.target) && !tabsNav?.contains(event.target)) cerrarMenuMobile();
  });
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape" && mobileNavToggle?.getAttribute("aria-expanded") === "true") {
      cerrarMenuMobile();
      mobileNavToggle?.focus();
    }
  });

  tabButtons.forEach(btn => {
    btn.addEventListener("click", () => {
      const pantalla = document.querySelector(".pantalla");
      const scrollActual = pantalla ? pantalla.scrollTop : 0;
      const tabAnterior = document.querySelector(".tab-content.activo")?.id;
      if (tabAnterior) scrollPorTab.set(tabAnterior, scrollActual);

      document.querySelectorAll(".tab-btn").forEach(b => b.classList.remove("activo"));
      document.querySelectorAll(".tab-content").forEach(c => c.classList.remove("activo"));

      btn.classList.add("activo");
      tabButtons.forEach((b) => {
        b.setAttribute("aria-selected", String(b === btn));
        b.tabIndex = b === btn ? 0 : -1;
      });
      if (mobileNavLabel) mobileNavLabel.textContent = btn.textContent.trim();
      cerrarMenuMobile();
      const targetId = btn.getAttribute("data-tab");
      const targetContent = document.getElementById(targetId);
      if (targetContent) {
        targetContent.classList.add("activo");

        actualizarHeaderColapsable();
        
        if (targetId === "tab-galab") {
          cargarAblacionGA();
        }
        if (targetId === "tab-logs" && !targetContent.dataset.researchLoaded) {
          targetContent.dataset.researchLoaded = "true";
          $("btnBacktest")?.click();
          $("btnLabSweep")?.click();
        }
        if (targetId === "tab-evidence"
          && targetContent.dataset.deferEvidence !== "true"
          && !targetContent.dataset.loaded) {
          targetContent.dataset.loaded = "true";
          cargarEvidenceLab();
        }

        // Forzar resize para que los canvas recalcule su bounding rect (ya que display: none devuelve width/height 0)
        requestAnimationFrame(() => {
          if (pantalla) {
            // Las pestañas visitadas recuperan su posición. Una pestaña nueva
            // empieza en su encabezado para no aparecer a media sección cuando
            // la vista anterior era más alta o mostraba el header de Resumen.
            const posicionGuardada = scrollPorTab.get(targetId);
            const inicioElemento = targetId === "tab-overview" ? headerDashboard() : tabsNav;
            const inicioTab = Math.max(0, (inicioElemento?.offsetTop ?? 18) - 18);
            pantalla.scrollTop = posicionGuardada ?? inicioTab;
          }
          window.dispatchEvent(new Event("resize"));
          window.dispatchEvent(new CustomEvent("mayab:tab-visible", {
            detail: { targetId },
          }));
        });
      }
    });
  });

  tabsNav?.addEventListener("keydown", (event) => {
    if (!event.target.classList.contains("tab-btn")) return;
    const actual = tabButtons.indexOf(event.target);
    if (actual < 0) return;
    let siguiente = actual;
    if (event.key === "ArrowRight") siguiente = (actual + 1) % tabButtons.length;
    else if (event.key === "ArrowLeft") siguiente = (actual - 1 + tabButtons.length) % tabButtons.length;
    else if (event.key === "Home") siguiente = 0;
    else if (event.key === "End") siguiente = tabButtons.length - 1;
    else return;
    event.preventDefault();
    tabButtons[siguiente].click();
    tabButtons[siguiente].focus();
  });

  document.querySelectorAll("[data-tab-target]").forEach((control) => {
    const activar = (event) => {
      const controlInteractivo = event.target.closest("a, button, input, select, textarea");
      if (event.type === "click" && controlInteractivo && controlInteractivo !== control) return;
      if (event.type === "click" && control.matches('a[href^="#"]')) event.preventDefault();
      const target = control.getAttribute("data-tab-target");
      const tab = document.querySelector(`.tab-btn[data-tab="${target}"]`);
      if (!tab) return;
      const abrirTab = () => {
        tab.click();
        // Usar el contenedor que realmente hace scroll evita dos animaciones
        // compitiendo entre sí, especialmente en Safari/Chrome móvil.
        // Las seis pruebas son navegación de precisión: su destino aparece en
        // el siguiente frame, sin un scroll suave que pueda dejar un lienzo
        // vacío a mitad de camino en Safari o en equipos lentos.
        requestAnimationFrame(() => irAlDashboard(!control.hasAttribute("data-jury-proof")));
      };
      if (control.hasAttribute("data-prepare-jury") && target === "tab-evidence") {
        const evidenceTab = document.getElementById(target);
        if (evidenceTab) evidenceTab.dataset.deferEvidence = "true";
        abrirTab();
        prepararDemoFinal().then(() => {
          if (!evidenceTab) return;
          delete evidenceTab.dataset.deferEvidence;
          if (!evidenceTab.dataset.loaded) {
            evidenceTab.dataset.loaded = "true";
            cargarEvidenceLab();
          }
        });
        return;
      }
      abrirTab();
      if (control.hasAttribute("data-prepare-jury")) prepararDemoFinal();
    };
    control.addEventListener("click", activar);
    if (!control.matches("a, button, input, select, textarea")) {
      control.addEventListener("keydown", (event) => {
        if (event.key !== "Enter" && event.key !== " ") return;
        event.preventDefault();
        activar(event);
      });
    }
  });

  if (pantalla) {
    pantalla.addEventListener("scroll", actualizarVisibilidadNotificaciones, { passive: true });
    pantalla.addEventListener("scrollend", guardarScroll, { passive: true });
    window.addEventListener("pagehide", guardarScroll, { passive: true });
  }
  actualizarHeaderColapsable();
  actualizarVisibilidadNotificaciones();
});

const MODAL_FOCUSABLE = [
  "a[href]", "button:not([disabled])", "input:not([disabled])", "select:not([disabled])",
  "textarea:not([disabled])", "[tabindex]:not([tabindex='-1'])",
].join(",");

function activarModalAccesible(panel, backdrop) {
  const exteriores = [...document.body.children]
    .filter((elemento) => elemento !== panel && elemento !== backdrop && !elemento.matches("script"))
    .map((elemento) => [elemento, elemento.inert]);
  exteriores.forEach(([elemento]) => { elemento.inert = true; });

  const enfocables = () => [...panel.querySelectorAll(MODAL_FOCUSABLE)]
    .filter((elemento) => !elemento.hidden && elemento.getAttribute("aria-hidden") !== "true");
  const confinarTab = (event) => {
    if (event.key !== "Tab") return;
    const items = enfocables();
    if (items.length === 0) {
      event.preventDefault();
      panel.focus();
      return;
    }
    const primero = items[0];
    const ultimo = items[items.length - 1];
    if (event.shiftKey && (document.activeElement === primero || !panel.contains(document.activeElement))) {
      event.preventDefault();
      ultimo.focus();
    } else if (!event.shiftKey && (document.activeElement === ultimo || !panel.contains(document.activeElement))) {
      event.preventDefault();
      primero.focus();
    }
  };
  const recuperarFoco = (event) => {
    if (!panel.hidden && !panel.contains(event.target)) enfocables()[0]?.focus();
  };
  panel.addEventListener("keydown", confinarTab);
  document.addEventListener("focusin", recuperarFoco);
  return () => {
    panel.removeEventListener("keydown", confinarTab);
    document.removeEventListener("focusin", recuperarFoco);
    exteriores.forEach(([elemento, previo]) => { elemento.inert = previo; });
  };
}

function iniciarDiccionario() {
  const toggle = $("dictionaryToggle");
  const panel = $("dictionaryPanel");
  const close = $("dictionaryClose");
  const backdrop = $("dictionaryBackdrop");
  const search = $("dictionarySearch");
  const rows = [...document.querySelectorAll("#dictionaryTerms tr")];
  const empty = $("dictionaryEmpty");
  if (!toggle || !panel || !close || !backdrop) return;

  rows.forEach((row, index) => row.style.setProperty("--dictionary-index", index));
  const reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)");
  let cierreTimer;
  let desactivarModal = () => {};

  const cerrar = () => {
    window.clearTimeout(cierreTimer);
    panel.classList.remove("is-open");
    backdrop.classList.remove("is-open");
    document.body.classList.remove("dictionary-open");
    toggle.setAttribute("aria-expanded", "false");
    desactivarModal();
    desactivarModal = () => {};
    cierreTimer = window.setTimeout(() => {
      panel.hidden = true;
      backdrop.hidden = true;
    }, reduceMotion.matches ? 0 : 430);
    toggle.focus();
  };
  const abrir = () => {
    window.clearTimeout(cierreTimer);
    panel.hidden = false;
    backdrop.hidden = false;
    desactivarModal = activarModalAccesible(panel, backdrop);
    document.body.classList.add("dictionary-open");
    toggle.setAttribute("aria-expanded", "true");
    requestAnimationFrame(() => {
      panel.classList.add("is-open");
      backdrop.classList.add("is-open");
      search?.focus();
    });
  };

  toggle.addEventListener("click", abrir);
  close.addEventListener("click", cerrar);
  backdrop.addEventListener("click", cerrar);
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape" && !panel.hidden) cerrar();
  });
  search?.addEventListener("input", () => {
    const query = search.value.trim().toLocaleLowerCase("es");
    let visibles = 0;
    rows.forEach((row) => {
      const coincide = row.textContent.toLocaleLowerCase("es").includes(query);
      row.hidden = !coincide;
      if (coincide) visibles += 1;
    });
    if (empty) empty.hidden = visibles !== 0;
  });
}

function iniciarTutorial() {
  const trigger = $("tutorialToggle");
  const panel = $("tutorialPanel");
  const backdrop = $("tutorialBackdrop");
  const close = $("tutorialClose");
  const prev = $("tutorialPrev");
  const next = $("tutorialNext");
  if (!trigger || !panel || !backdrop || !close || !prev || !next) return;

  const pasos = [
    {
      tab: "tab-overview",
      selector: ".llm-strip",
      titulo: "Resumen de la sesión",
      texto: "Consulta el modo de datos, PnL, riesgo, mejor ruta y estado del GA antes de entrar al detalle.",
      prueba: () => `${ultimoEstado?.cotizaciones?.length || 0} feeds utilizables · ${ultimoEstado?.metricas?.operaciones || 0} operaciones simuladas`,
    },
    {
      tab: "tab-mercado",
      selector: ".mercado",
      titulo: "Feeds y procedencia",
      texto: "Los WebSocket normalizan bid, ask, profundidad, timestamp y fuente. El fallback REST se identifica por separado.",
      prueba: () => `${coberturaMercado(ultimoEstado || {}).wsFrescos} WebSockets frescos · ${formato(ultimoEstado?.metricas?.latenciaPromedioMs || 0, 0)} ms promedio`,
    },
    {
      tab: "tab-mercado",
      selector: ".mapa",
      titulo: "De spread bruto a utilidad neta",
      texto: "Cada ruta descuenta fees, slippage, retiro amortizado y riesgo de latencia; después limita el tamaño por profundidad e inventario.",
      prueba: () => {
        const ruta = ultimoEstado?.oportunidades?.[0];
        return ruta ? `${ruta.compraEn} → ${ruta.ventaEn} · ${formato(ruta.diferencialNetoBps || 0, 2)} bps netos` : "Esperando una ruta auditable";
      },
    },
    {
      tab: "tab-riesgo",
      selector: ".demo-panel",
      titulo: "Pruebas de riesgo y recuperación",
      texto: "Ejecuta fill parcial, fallo de orden, shock de mercado, circuit breaker y rebalanceo como escenarios controlados.",
      prueba: () => `${ultimoEstado?.metricas?.operacionesFallidas || 0} fallos · ${ultimoEstado?.metricas?.rebalanceosTotales || 0} rebalanceos auditados`,
    },
    {
      tab: "tab-logs",
      selector: ".replay-panel",
      titulo: "Validación contra el baseline",
      texto: "El replay evalúa el campeón GA y reporta mediana, P05–P95 e intervalo de confianza sobre 24 semillas comunes.",
      prueba: () => "Mismas condiciones · múltiples semillas · resultado sujeto al intervalo de confianza",
    },
    {
      tab: "tab-galab",
      selector: ".ga-panel",
      titulo: "Optimización y holdout",
      texto: "El GA ajusta pesos, umbral, tamaño y tolerancia. Solo supera al baseline si la mejora se mantiene en el holdout.",
      prueba: () => `Generación ${ultimoEstado?.genetico?.generacion || 0} · población ${ultimoEstado?.genetico?.poblacion || 0} · diversidad ${formato((ultimoEstado?.genetico?.diversidad || 0) * 100, 1)}%`,
    },
  ];
  let indice = 0;
  let resaltado = null;
  let desactivarModal = () => {};

  const limpiarResaltado = () => {
    resaltado?.classList.remove("tutorial-highlight");
    resaltado = null;
  };

  const mostrarPaso = () => {
    const paso = pasos[indice];
    document.querySelector(`.tab-btn[data-tab="${paso.tab}"]`)?.click();
    limpiarResaltado();
    requestAnimationFrame(() => {
      resaltado = document.querySelector(`#${paso.tab} ${paso.selector}`) || document.querySelector(paso.selector);
      resaltado?.classList.add("tutorial-highlight");
      resaltado?.scrollIntoView({ behavior: reducirMovimiento ? "auto" : "smooth", block: "center" });
    });
    setText("tutorialStepLabel", `Paso ${indice + 1} de ${pasos.length}`);
    setText("tutorialTitle", paso.titulo);
    setText("tutorialText", paso.texto);
    setText("tutorialProof", paso.prueba());
    $("tutorialProgressBar")?.style.setProperty("width", `${((indice + 1) / pasos.length) * 100}%`);
    prev.disabled = indice === 0;
    next.textContent = indice === pasos.length - 1 ? "Terminar" : "Siguiente";
  };

  const cerrar = () => {
    panel.hidden = true;
    backdrop.hidden = true;
    document.body.classList.remove("tutorial-open");
    limpiarResaltado();
    desactivarModal();
    desactivarModal = () => {};
    trigger.focus();
  };
  const abrir = () => {
    indice = 0;
    panel.hidden = false;
    backdrop.hidden = false;
    desactivarModal = activarModalAccesible(panel, backdrop);
    document.body.classList.add("tutorial-open");
    mostrarPaso();
    requestAnimationFrame(() => next.focus());
  };

  trigger.addEventListener("click", abrir);
  close.addEventListener("click", cerrar);
  backdrop.addEventListener("click", cerrar);
  prev.addEventListener("click", () => {
    if (indice > 0) indice -= 1;
    mostrarPaso();
  });
  next.addEventListener("click", () => {
    if (indice >= pasos.length - 1) {
      cerrar();
      return;
    }
    indice += 1;
    mostrarPaso();
  });
  document.addEventListener("keydown", (event) => {
    if (panel.hidden) return;
    if (event.key === "Escape") cerrar();
    if (event.key === "ArrowRight") next.click();
    if (event.key === "ArrowLeft") prev.click();
  });
}

function iniciarLanding() {
  // Solo las cards marcadas en el HTML participan en el reveal por scroll.
  // Paneles, metricas, graficas y navegacion conservan sus animaciones propias
  // de entrada, pero no vuelven a un estado oculto mientras esperan al observer.
  const elementsToReveal = document.querySelectorAll(".reveal-card");
  const ordenPorGrupo = new Map();
  elementsToReveal.forEach(el => {
    const grupo = el.parentElement;
    const orden = ordenPorGrupo.get(grupo) || 0;
    el.style.setProperty("--reveal-delay", `${Math.min(orden * 45, 135)}ms`);
    // Variar apenas el punto de expansión evita que toda la cuadrícula revele
    // como un bloque y refuerza el efecto de card compacta a card completa.
    const origenes = ["46% 58%", "54% 54%", "50% 62%"];
    el.style.setProperty("--reveal-origin", origenes[orden % origenes.length]);
    ordenPorGrupo.set(grupo, orden + 1);
  });

  const cards = document.querySelectorAll(".reveal-card");
  // El primer viewport es contenido, no un loading state. Dejarlo visible desde
  // el primer frame evita que una carga lenta del modulo o una peculiaridad del
  // IntersectionObserver de Safari convierta el hero en bloques borrosos.
  document
    .querySelectorAll(".landing-hero .reveal-card")
    .forEach((card) => card.classList.add("is-visible"));
  // La clase que permite ocultar cards entra sólo después de revelar el hero.
  // Si el módulo falla antes de este punto, el HTML permanece visible como
  // fallback progresivo en vez de convertirse en una pantalla en blanco.
  document.documentElement.classList.add("reveal-ready");
  const ctas = document.querySelectorAll('a[href="#dashboard"]');
  ctas.forEach((cta) => {
    cta.addEventListener("click", (event) => {
      event.preventDefault();
      irAlDashboard();
    });
  });

  if (location.hash === "#dashboard") {
    requestAnimationFrame(() => irAlDashboard(false));
  }

  if (!cards.length) return;
  if (!("IntersectionObserver" in window)) {
    cards.forEach((card) => card.classList.add("is-visible"));
    return;
  }

  const root = document.querySelector(".pantalla");
  const observer = new IntersectionObserver(
    (entries) => {
      entries
        .filter((entry) => entry.isIntersecting)
        .sort((a, b) => a.boundingClientRect.top - b.boundingClientRect.top)
        .forEach((entry) => {
          entry.target.classList.add("is-visible");
          observer.unobserve(entry.target);
        });
    },
    // La zona inferior ampliada inicia el reveal antes de que el usuario tenga
    // que llevar una tarjeta grande hasta el centro del viewport.
    { root, threshold: 0.04, rootMargin: "8% 0px 28% 0px" },
  );

  cards.forEach((card) => observer.observe(card));

  const revelarCercanas = () => {
    const limite = root?.getBoundingClientRect() || {
      top: 0,
      bottom: window.innerHeight,
      height: window.innerHeight,
    };
    const anticipacion = limite.height * 0.28;
    cards.forEach((card) => {
      if (card.classList.contains("is-visible") || card.offsetParent === null) return;
      const rect = card.getBoundingClientRect();
      if (rect.bottom >= limite.top && rect.top <= limite.bottom + anticipacion) {
        card.classList.add("is-visible");
        observer.unobserve(card);
      }
    });
  };

  const revelarTabActivo = (event) => {
    const targetId = event?.detail?.targetId;
    const contenido = targetId
      ? document.getElementById(targetId)
      : document.querySelector(".tab-content.activo");
    if (!contenido?.classList.contains("activo")) return;

    // IntersectionObserver puede conservar la medición de cuando el tab tenía
    // display:none. Al entrar a una vista, hacer visible su contenido de forma
    // explícita evita un panel blanco hasta el siguiente scroll o hover.
    contenido.querySelectorAll(".reveal-card:not(.is-visible)").forEach((card) => {
      card.classList.add("is-visible");
      observer.unobserve(card);
    });
    revelarCercanas();
  };

  requestAnimationFrame(revelarCercanas);
  root?.addEventListener("scroll", revelarCercanas, { passive: true });
  // Cinturón de seguridad para observers pausados por ahorro de batería,
  // cambios de pestaña o WebViews: ninguna card queda bloqueada para siempre.
  window.setTimeout(() => {
    cards.forEach((card) => card.classList.add("is-visible"));
  }, 1600);
  window.addEventListener("mayab:tab-visible", revelarTabActivo);
}

function irAlDashboard(suave = true) {
  const pantalla = document.querySelector(".pantalla");
  const tabsNav = document.querySelector(".tabs-nav");
  const header = document.querySelector(".barra-superior");
  if (!pantalla) return;

  const activo = document.querySelector(".tab-content.activo");
  const esOverview = !activo || activo.id === "tab-overview";
  const target = esOverview ? header : tabsNav;
  if (!target) return;

  actualizarHeaderColapsable();
  pantalla.scrollTo({
    top: Math.max(0, target.offsetTop - 18),
    behavior: suave ? "smooth" : "auto",
  });
  history.replaceState(null, "", "#dashboard");
}

function headerDashboard() {
  return document.querySelector(".barra-superior");
}

function iniciarHeaderColapsable() {
  if (document.readyState === "loading") return;
  actualizarHeaderColapsable();
}

function actualizarVisibilidadNotificaciones() {
  const pantalla = document.querySelector(".pantalla");
  const dashboard = document.getElementById("dashboard");
  const container = $("notificaciones");
  if (!pantalla || !dashboard || !container) return;
  const dashboardVisible = pantalla.scrollTop >= Math.max(0, dashboard.offsetTop - 80);
  container.classList.toggle("is-visible", dashboardVisible);
}

function actualizarHeaderColapsable() {
  const pantalla = document.querySelector(".pantalla");
  const header = document.querySelector(".barra-superior");
  const activo = document.querySelector(".tab-content.activo");
  if (!pantalla || !header) return;

  const esOverview = activo?.id === "tab-overview";
  header.style.display = esOverview ? "" : "none";
  pantalla.classList.toggle("header-fuera-tab", !esOverview);
  // El encabezado conserva siempre su altura: el titulo y el grid son parte de
  // la identidad visual del dashboard, no un elemento que se contrae al leer.
  pantalla.classList.remove("header-colapsado");
}
