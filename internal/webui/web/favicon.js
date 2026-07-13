// Favicon SVG reactivo de Mayab Edge.
// El navegador conserva el formato vectorial; no se rasteriza mediante canvas.
const DEBUG_ACTIVO =
  new URLSearchParams(location.search).get("debug") === "1" ||
  localStorage.getItem("mayabDebug") === "1";

class FaviconAnimator {
  constructor() {
    this.linkEl = document.getElementById("favicon") || document.querySelector("link[rel~='icon']");
    this.estadoSocket = "conectando";
    this.socketOk = undefined;
    this.resetGananciaId = null;
    this.movimientoReducido = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

    if (!this.linkEl) return;

    window.addEventListener("mayab:socket", (evento) => {
      this.estadoSocket = evento.detail?.texto || "conectando";
      this.socketOk = evento.detail?.ok;
      this.renderizar(false);
    });

    window.addEventListener("mayab:arbitraje", () => this.animarGanancia());
    this.renderizar(false);

    if (DEBUG_ACTIVO) console.log("[Favicon] SVG reactivo inicializado.");
  }

  animarGanancia() {
    window.clearTimeout(this.resetGananciaId);
    this.renderizar(true);
    this.resetGananciaId = window.setTimeout(() => this.renderizar(false), 1900);
  }

  colores(ganancia) {
    if (ganancia) return { principal: "#22c55e", secundario: "#facc15", fondo: "#052e16" };
    if (this.socketOk === false) return { principal: "#ef4444", secundario: "#fb7185", fondo: "#2b0b12" };
    if (this.socketOk === undefined || ["conectando", "reconectando"].includes(this.estadoSocket)) {
      return { principal: "#f59e0b", secundario: "#f97316", fondo: "#2a1605" };
    }
    return { principal: "#f7931a", secundario: "#22c55e", fondo: "#071a12" };
  }

  crearSvg(ganancia) {
    const { principal, secundario, fondo } = this.colores(ganancia);
    const animado = !this.movimientoReducido;
    const animacionColor = animado
      ? `<animate attributeName="stop-color" values="${principal};${secundario};${principal}" dur="2.4s" repeatCount="indefinite"/>`
      : "";

    return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32">
      <defs>
        <linearGradient id="g" x1="4" y1="28" x2="28" y2="4" gradientUnits="userSpaceOnUse">
          <stop stop-color="${principal}">${animacionColor}</stop>
          <stop offset="1" stop-color="${secundario}"/>
        </linearGradient>
      </defs>
      <style>
        .aro{transform-origin:16px 16px;${animado ? "animation:pulso 1.8s ease-in-out infinite" : ""}}
        .grafica{stroke-dasharray:29;stroke-dashoffset:${ganancia && animado ? "29;animation:sube .72s ease-out forwards" : "0"}}
        .punta{transform-origin:25px 7px;${ganancia && animado ? "animation:salta .72s ease-out" : ""}}
        .btc{transform-origin:16px 16px;${ganancia && animado ? "animation:moneda .72s ease-out" : ""}}
        @keyframes pulso{50%{opacity:.6;transform:scale(.94)}}
        @keyframes sube{to{stroke-dashoffset:0}}
        @keyframes salta{0%,45%{opacity:0;transform:scale(.3)}100%{opacity:1;transform:scale(1)}}
        @keyframes moneda{45%{transform:translateY(-1.5px) scale(1.12)}}
      </style>
      <rect width="32" height="32" rx="8" fill="${fondo}"/>
      <circle class="aro" cx="16" cy="16" r="12.5" fill="none" stroke="url(#g)" stroke-width="1.5" opacity=".9"/>
      <circle cx="13.5" cy="16" r="7.4" fill="url(#g)"/>
      <path class="btc" fill="#fff" d="M11.1 10.8h1.2V9.5h1.2v1.3h.9V9.5h1.2v1.4c1.6.3 2.5 1.1 2.5 2.4 0 .9-.4 1.6-1.2 2 1 .4 1.6 1.2 1.6 2.4 0 1.7-1.2 2.8-3 3.1v1.6h-1.2v-1.5h-1v1.5h-1.2v-1.5h-1.3v-1.5h1.1v-7.1h-.8v-1.5Zm3 4c1.5 0 2.3-.3 2.3-1.2 0-.8-.6-1.1-1.9-1.1h-.8v2.3h.4Zm.3 4.4c1.6 0 2.4-.4 2.4-1.4 0-.9-.7-1.3-2.5-1.3h-.6v2.7h.7Z"/>
      <path class="grafica" d="M18.5 21.5 21 18l2 1 3.5-5" fill="none" stroke="#dcfce7" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>
      <path class="punta" d="m24 14 2.8-.4-.3 2.8" fill="none" stroke="#dcfce7" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/>
    </svg>`;
  }

  renderizar(ganancia) {
    const svg = this.crearSvg(ganancia);
    this.linkEl.type = "image/svg+xml";
    this.linkEl.href = `data:image/svg+xml,${encodeURIComponent(svg)}`;
  }
}

if (document.readyState === "loading") {
  document.addEventListener("DOMContentLoaded", () => new FaviconAnimator(), { once: true });
} else {
  new FaviconAnimator();
}
