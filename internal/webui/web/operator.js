"use strict";
const byId=(id)=>document.getElementById(id);
const number=new Intl.NumberFormat("es-MX",{maximumFractionDigits:2});
const money=new Intl.NumberFormat("es-MX",{style:"currency",currency:"USD"});
function metric(data,key,fallback=0){return data.metricas?.[key]??fallback}
function renderDefinitions(target,items){target.replaceChildren(...items.flatMap(([term,value])=>{const dt=document.createElement("dt");dt.textContent=term;const dd=document.createElement("dd");dd.textContent=String(value);return[dt,dd]}))}
function render(data){
  const connected=(data.cotizaciones||[]).filter((quote)=>quote.conectado).length;
  byId("pnl").textContent=money.format(metric(data,"utilidadAcumuladaUsd"));
  byId("operations").textContent=number.format(metric(data,"operaciones"));
  byId("feeds").textContent=`${connected}/${(data.cotizaciones||[]).length}`;
  byId("latency").textContent=`${number.format(metric(data,"latenciaPromedioMs"))} ms`;
  byId("opportunities").textContent=number.format((data.oportunidades||[]).length);
  byId("ga").textContent=data.genetico?`Gen ${data.genetico.generacion}`:"Inactivo";
  const circuit=Boolean(metric(data,"circuitBreakerActivo"));
  const banner=byId("banner"); banner.className=`banner ${circuit?"bad":"ok"}`;
  banner.textContent=circuit?"Circuit breaker activo · ejecución simulada detenida":"Motor operativo · simulación sin fondos reales";
  const persistence=data.persistencia||{};
  const queueHealth=(persistence.queueDropped||0)+(persistence.queueFailed||0)===0
    ? `cola sana · ${number.format(persistence.queuePending||0)} pendientes`
    : `degradada · ${number.format(persistence.queueDropped||0)} descartadas · ${number.format(persistence.queueFailed||0)} fallidas`;
  renderDefinitions(byId("risk"),[["Estado de riesgo",metric(data,"estadoRiesgo","sin dato")],["Drawdown máximo",money.format(metric(data,"maxDrawdownUsd"))],["Persistencia",`${persistence.storageStatus||"sin estado"} · ${queueHealth}`],["Rebalanceos",number.format(metric(data,"rebalanceosTotales"))]]);
  byId("exchanges").replaceChildren(...Object.entries(data.exchangesActivos||{}).sort().map(([name,on])=>{const el=document.createElement("span");el.className=`pill ${on?"on":""}`;el.textContent=`${name} · ${on?"ON":"OFF"}`;return el}));
  byId("events").replaceChildren(...(data.eventosEjecucion||[]).slice(-8).reverse().map((event)=>{const el=document.createElement("li");el.textContent=event.descripcion||event.detalle||event.tipo||"Evento del motor";return el}));
}
async function refresh(){try{const response=await fetch("/api/estado",{cache:"no-store"});if(!response.ok)throw new Error(`HTTP ${response.status}`);render(await response.json())}catch(error){const banner=byId("banner");banner.className="banner bad";banner.textContent=`Estado no disponible · ${error.message}`}}
refresh();setInterval(refresh,2000);
