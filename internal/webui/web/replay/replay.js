const $ = (id) => document.getElementById(id);
const start = $("start");
const stop = $("stop");
const run = $("run");

function mutationHeaders() {
  const headers = { "Content-Type": "application/json" };
  const token = localStorage.getItem("mayabAdminToken");
  if (token) headers.Authorization = `Bearer ${token}`;
  return headers;
}

async function request(url) {
  const response = await fetch(url, { method: "POST", headers: mutationHeaders() });
  const body = await response.json().catch(() => ({}));
  if (!response.ok || body.ok === false) throw new Error(body.error || `HTTP ${response.status}`);
  return body;
}

function renderState(state) {
  $("snapshots").textContent = Number(state.snapshots || 0).toLocaleString("es-MX");
  $("duration").textContent = `${state.duracionSegundos || 0} s`;
  $("dot").classList.toggle("active", state.activa === true);
  $("status").textContent = state.activa
    ? "Capturando cotizaciones públicas"
    : state.snapshots > 0 ? "Tape listo para replay" : "Listo para capturar";
  start.disabled = state.activa;
  stop.disabled = !state.activa;
  run.disabled = state.activa || state.snapshots === 0;
}

async function refresh() {
  try {
    const response = await fetch("/api/replay/captura/estado");
    if (response.ok) renderState(await response.json());
  } catch { $("status").textContent = "No se pudo consultar el servidor"; }
}

start.onclick = async () => {
  start.disabled = true;
  $("status").textContent = "Iniciando captura…";
  try { await request("/api/replay/captura/iniciar"); await refresh(); }
  catch (error) { $("status").textContent = error.message; start.disabled = false; }
};
stop.onclick = async () => {
  stop.disabled = true;
  $("status").textContent = "Cerrando tape…";
  try { await request("/api/replay/captura/detener"); await refresh(); }
  catch (error) { $("status").textContent = error.message; stop.disabled = false; }
};
run.onclick = async () => {
  run.disabled = true;
  $("resultTitle").textContent = "Ejecutando motor aislado…";
  try {
    const result = await request("/api/replay/ejecutar");
    $("resultTitle").textContent = "Replay completado";
    $("resultGrid").classList.remove("muted");
    const hash = typeof result.inputSha256 === "string" ? result.inputSha256 : "sin-huella";
    $("resultGrid").innerHTML = `<article><span>Ticks</span><strong>${Number(result.ticksProcesados).toLocaleString("es-MX")}</strong></article><article><span>Operaciones</span><strong>${Number(result.operaciones).toLocaleString("es-MX")}</strong></article><article><span>PnL simulado</span><strong>$${Number(result.pnlUsd).toLocaleString("es-MX", {minimumFractionDigits: 2, maximumFractionDigits: 2})}</strong></article><article title="${hash}"><span>Input SHA-256</span><strong>${hash.slice(0, 12)}…</strong></article>`;
    $("resultNote").textContent = `${result.mensaje} · reloj del tape · adversidad aleatoria desactivada`;
  } catch (error) { $("resultTitle").textContent = "No se pudo ejecutar"; $("resultNote").textContent = error.message; }
  finally { await refresh(); }
};

refresh();
setInterval(refresh, 2000);
