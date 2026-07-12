//! Discord Interactions + agente NVIDIA con tools del simulador.

use std::{sync::Arc, time::Duration};

use axum::{
    body::Bytes,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    motor::{EscenarioDemo, Motor},
    types::EstadoPublico,
};

pub const SIGNATURE_HEADER: &str = "x-signature-ed25519";
pub const TIMESTAMP_HEADER: &str = "x-signature-timestamp";

#[derive(Clone, Default)]
pub struct ConfigDiscord {
    pub application_id: Option<String>,
    pub public_key: Option<VerifyingKey>,
    pub bot_token: Option<String>,
    pub guild_id: Option<String>,
    nvidia: Option<ConfigNvidia>,
}

#[derive(Clone)]
struct ConfigNvidia {
    api_key: String,
    models: Vec<String>,
}

impl ConfigDiscord {
    pub fn from_env() -> Self {
        let public_key = env_optional("DISCORD_PUBLIC_KEY").and_then(|value| {
            let bytes = hex::decode(&value)
                .map_err(|e| e.to_string())
                .and_then(|bytes| {
                    <[u8; 32]>::try_from(bytes).map_err(|_| "debe contener 32 bytes".into())
                });
            match bytes
                .and_then(|bytes| VerifyingKey::from_bytes(&bytes).map_err(|e| e.to_string()))
            {
                Ok(key) => Some(key),
                Err(error) => {
                    tracing::warn!(%error, "DISCORD_PUBLIC_KEY invalida; bot desactivado");
                    None
                }
            }
        });
        let nvidia = env_optional("NVIDIA_API_KEY").map(|api_key| ConfigNvidia {
            api_key,
            models: env_optional("NVIDIA_MODELS")
                .unwrap_or_else(|| "nvidia/nemotron-3-nano-omni-30b-a3b-reasoning,nvidia/nemotron-3-nano-30b-a3b,nvidia/nemotron-3-ultra-550b-a55b".into())
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect(),
        });
        Self {
            application_id: env_optional("DISCORD_APPLICATION_ID"),
            public_key,
            bot_token: env_optional("DISCORD_BOT_TOKEN"),
            guild_id: env_optional("DISCORD_GUILD_ID"),
            nvidia,
        }
    }

    pub fn habilitado(&self) -> bool {
        self.public_key.is_some()
    }

    pub fn registrar_estado(&self) {
        tracing::info!(
            interactions = self.public_key.is_some(),
            application_id = self.application_id.is_some(),
            bot_token = self.bot_token.is_some(),
            guild_id = self.guild_id.is_some(),
            nvidia = self.nvidia.is_some(),
            "configuracion de Discord cargada"
        );
    }
}

#[derive(Debug, Deserialize)]
struct Interaccion {
    #[serde(rename = "type")]
    tipo: u8,
    #[serde(default)]
    data: Option<DatosComando>,
    #[serde(default)]
    token: String,
    #[serde(default)]
    application_id: String,
    #[serde(default)]
    member: Option<Miembro>,
}

#[derive(Debug, Deserialize)]
struct DatosComando {
    name: String,
    #[serde(default)]
    options: Vec<OpcionComando>,
}

#[derive(Debug, Deserialize)]
struct OpcionComando {
    name: String,
    value: Value,
}

#[derive(Debug, Deserialize)]
struct Miembro {
    #[serde(default)]
    permissions: String,
}

pub async fn responder_interaccion(
    motor: Arc<Motor>,
    config: &ConfigDiscord,
    headers: &HeaderMap,
    body: Bytes,
) -> Response {
    let Some(public_key) = config.public_key.as_ref() else {
        tracing::warn!("interaccion de Discord rechazada: falta DISCORD_PUBLIC_KEY");
        return (StatusCode::SERVICE_UNAVAILABLE, "Discord no configurado").into_response();
    };
    if !firma_valida(public_key, headers, &body) {
        tracing::warn!(
            tiene_firma = headers.contains_key(SIGNATURE_HEADER),
            tiene_timestamp = headers.contains_key(TIMESTAMP_HEADER),
            body_bytes = body.len(),
            "interaccion de Discord rechazada: firma invalida"
        );
        return (StatusCode::UNAUTHORIZED, "Firma de Discord invalida").into_response();
    }
    let interaccion: Interaccion = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(%error, body_bytes = body.len(), "payload de Discord invalido");
            return (StatusCode::BAD_REQUEST, "Payload invalido").into_response();
        }
    };
    tracing::info!(
        tipo = interaccion.tipo,
        comando = interaccion.data.as_ref().map(|data| data.name.as_str()),
        "interaccion de Discord validada"
    );
    if interaccion.tipo == 1 {
        return Json(json!({ "type": 1 })).into_response();
    }
    if interaccion.tipo != 2 {
        return respuesta("Tipo de interaccion no soportado.", true);
    }
    let Some(data) = interaccion.data else {
        return respuesta("No se recibio un comando.", true);
    };
    if data.name != "mayab" && data.name != "ask" {
        return ejecutar_comando(&motor, &data.name).await;
    }
    let Some(nvidia) = config.nvidia.clone() else {
        return respuesta("La IA no esta configurada: falta NVIDIA_API_KEY.", true);
    };
    let pregunta = data
        .options
        .iter()
        .find(|option| option.name == "pregunta")
        .and_then(|option| option.value.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if pregunta.is_empty() {
        return respuesta("Escribe una pregunta para Mayab IA.", true);
    }
    let puede_mutar = interaccion
        .member
        .as_ref()
        .is_some_and(|member| permisos_admin(&member.permissions));
    let application_id = interaccion.application_id;
    let token = interaccion.token;
    tokio::spawn(async move {
        let resultado = agente_nvidia(motor, nvidia, pregunta, puede_mutar).await;
        completar_interaccion(&application_id, &token, &resultado).await;
    });
    Json(json!({ "type": 5, "data": { "content": "Mayab IA esta analizando..." } })).into_response()
}

fn firma_valida(public_key: &VerifyingKey, headers: &HeaderMap, body: &[u8]) -> bool {
    let Some(timestamp) = headers.get(TIMESTAMP_HEADER).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    let Some(signature) = headers.get(SIGNATURE_HEADER).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    let Ok(signature) = hex::decode(signature).and_then(|v| {
        Signature::from_slice(&v).map_err(|_| hex::FromHexError::InvalidStringLength)
    }) else {
        return false;
    };
    let mut message = Vec::with_capacity(timestamp.len() + body.len());
    message.extend_from_slice(timestamp.as_bytes());
    message.extend_from_slice(body);
    public_key.verify(&message, &signature).is_ok()
}

fn permisos_admin(value: &str) -> bool {
    value
        .parse::<u64>()
        .is_ok_and(|bits| bits & ((1 << 3) | (1 << 5)) != 0)
}

async fn ejecutar_comando(motor: &Motor, nombre: &str) -> Response {
    match nombre {
        "estado" | "resumen" => respuesta(&resumen_estado(&motor.estado().await), false),
        "demo-rentable" => {
            motor.evolucionar_ga(true, 96).await;
            motor
                .activar_escenario_demo(EscenarioDemo::MercadoRentable)
                .await;
            respuesta(
                &format!(
                    "Demo simulada preparada.\n{}",
                    resumen_estado(&motor.estado().await)
                ),
                false,
            )
        }
        _ => respuesta("Comando desconocido.", true),
    }
}

fn resumen_estado(estado: &EstadoPublico) -> String {
    let ga = estado.genetico.as_ref().map_or_else(
        || "inactivo".into(),
        |ga| {
            format!(
                "generacion {} · fitness {:.2}",
                ga.generacion, ga.mejor_fitness
            )
        },
    );
    format!("**Mayab Arbitraje BTC** (simulacion)\nPnL: **${:.2} USD** · retorno: **{:.2} bps**\nOperaciones: **{}** · riesgo: **{}**\nGA: {} · feeds: {} cotizaciones", estado.metricas.utilidad_acumulada_usd, estado.metricas.retorno_bps, estado.metricas.operaciones, estado.metricas.estado_riesgo, ga, estado.cotizaciones.len())
}

fn respuesta(content: &str, ephemeral: bool) -> Response {
    let mut data = json!({ "content": content, "allowed_mentions": { "parse": [] } });
    if ephemeral {
        data["flags"] = json!(64);
    }
    Json(json!({ "type": 4, "data": data })).into_response()
}

pub async fn registrar_comandos(config: ConfigDiscord) {
    let (Some(application_id), Some(bot_token)) = (config.application_id, config.bot_token) else {
        tracing::info!(
            "Discord disponible; falta Application ID o Bot Token para registrar comandos"
        );
        return;
    };
    let base = format!("https://discord.com/api/v10/applications/{application_id}");
    let url = config.guild_id.map_or_else(
        || format!("{base}/commands"),
        |guild| format!("{base}/guilds/{guild}/commands"),
    );
    tracing::info!(
        application_id = %application_id,
        alcance = if url.contains("/guilds/") { "guild" } else { "global" },
        "iniciando registro de slash commands"
    );
    let commands = [
        json!({"name":"estado","type":1,"description":"Muestra PnL, riesgo, operaciones y GA"}),
        json!({"name":"resumen","type":1,"description":"Resumen compacto de Mayab Arbitraje BTC"}),
        json!({"name":"demo-rentable","type":1,"description":"Prepara el escenario rentable estrictamente simulado"}),
        json!({"name":"mayab","type":1,"description":"Consulta a la IA o ajusta la simulacion con tools","options":[{"name":"pregunta","description":"Ejemplo: muestra el riesgo o cambia el slippage a 0.5 bps","type":3,"required":true,"max_length":600}]}),
        json!({"name":"ask","type":1,"description":"Pregunta cualquier cosa a Mayab IA","options":[{"name":"pregunta","description":"Pregunta general o sobre datos y configuracion de Mayab","type":3,"required":true,"max_length":1200}]}),
    ];
    let client = reqwest::Client::new();
    for command in commands {
        match client
            .post(&url)
            .header("Authorization", format!("Bot {bot_token}"))
            .json(&command)
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => {
                tracing::info!(comando=%command["name"], "slash command registrado")
            }
            Ok(response) => {
                let status = response.status();
                let detalle = response
                    .text()
                    .await
                    .unwrap_or_else(|_| "respuesta ilegible".into());
                tracing::warn!(
                    comando = %command["name"],
                    %status,
                    detalle = %truncar_log(&detalle),
                    "Discord rechazo el comando"
                )
            }
            Err(error) => {
                tracing::warn!(comando=%command["name"], %error, "fallo el registro del comando")
            }
        }
    }
}

async fn agente_nvidia(
    motor: Arc<Motor>,
    config: ConfigNvidia,
    question: String,
    can_mutate: bool,
) -> String {
    let mut messages = vec![
        json!({"role":"system","content":"Eres Mayab IA, un asistente general dentro de Discord. Puedes responder preguntas generales con tu conocimiento y preguntas sobre Mayab Arbitraje BTC usando siempre las tools disponibles para datos actuales. Mayab es una demo estrictamente simulada: nunca afirmes trading real. La auditoria es de solo lectura. Solo cambia parametros si el usuario lo pide explicitamente y la tool esta disponible; explica brevemente cualquier cambio. No inventes datos que una tool pueda consultar. Responde en el idioma del usuario, de forma clara y concisa."}),
        json!({"role":"user","content":question}),
    ];
    let tools = tools_nvidia(can_mutate);
    let (mut model, mut message) = match chat_fallback(&config, &messages, &tools).await {
        Ok(value) => value,
        Err(error) => return format!("No pude consultar NVIDIA: {error}"),
    };
    if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
        let calls = calls.iter().take(4).cloned().collect::<Vec<_>>();
        messages.push(message);
        for call in calls {
            let id = call["id"].as_str().unwrap_or("tool");
            let name = call["function"]["name"].as_str().unwrap_or("");
            let args = call["function"]["arguments"]
                .as_str()
                .and_then(|v| serde_json::from_str(v).ok())
                .unwrap_or_else(|| json!({}));
            let result = ejecutar_tool_ia(&motor, name, args, can_mutate).await;
            messages.push(json!({"role":"tool","tool_call_id":id,"content":result.to_string()}));
        }
        match chat_fallback(&config, &messages, &tools).await {
            Ok(value) => {
                model = value.0;
                message = value.1;
            }
            Err(error) => {
                return format!("Las tools funcionaron, pero fallo la respuesta: {error}")
            }
        }
    }
    let content = message["content"]
        .as_str()
        .unwrap_or("Sin respuesta del modelo.");
    truncar(&format!("{content}\n\n_Modelo: {model}_"))
}

fn tools_nvidia(can_mutate: bool) -> Vec<Value> {
    let mut tools = vec![
        tool(
            "get_state",
            "Obtiene metricas, riesgo, operaciones y GA",
            json!({"type":"object","properties":{}}),
        ),
        tool(
            "get_config",
            "Obtiene parametros actuales",
            json!({"type":"object","properties":{}}),
        ),
        tool(
            "get_audit_history",
            "Consulta en modo solo lectura el resumen de SQLite y las ultimas 20 operaciones simuladas",
            json!({"type":"object","properties":{}}),
        ),
        tool(
            "prepare_demo",
            "Prepara una demo rentable simulada",
            json!({"type":"object","properties":{}}),
        ),
    ];
    if can_mutate {
        tools.push(tool("update_parameters", "Cambia parametros simples del simulador", json!({"type":"object","properties":{"maxOperacionBtc":{"type":"number"},"minDiferencialNetoBps":{"type":"number"},"deslizamientoBps":{"type":"number"},"minUtilidadUsd":{"type":"number"},"enfriamientoMs":{"type":"integer"}},"additionalProperties":false})));
    }
    tools
}

fn tool(name: &str, description: &str, parameters: Value) -> Value {
    json!({"type":"function","function":{"name":name,"description":description,"parameters":parameters}})
}

async fn ejecutar_tool_ia(motor: &Motor, name: &str, args: Value, can_mutate: bool) -> Value {
    match name {
        "get_state" => {
            let state = motor.estado().await;
            json!({"generadoEn":state.generado_en,"metricas":state.metricas,"genetico":state.genetico,"cotizaciones":state.cotizaciones.len()})
        }
        "get_config" => json!(motor.estado().await.configuracion),
        "get_audit_history" => motor.resumen_auditoria(),
        "prepare_demo" => {
            json!({"ok":true,"resultado":motor.activar_escenario_demo(EscenarioDemo::MercadoRentable).await})
        }
        "update_parameters" if can_mutate => {
            let mut cfg = motor.estado().await.configuracion;
            if let Some(v) = bounded(&args, "maxOperacionBtc", 0.000001, 10.0) {
                cfg.max_operacion_btc = v;
            }
            if let Some(v) = bounded(&args, "minDiferencialNetoBps", 0.0, 10_000.0) {
                cfg.min_diferencial_neto_bps = v;
            }
            if let Some(v) = bounded(&args, "deslizamientoBps", 0.0, 1_000.0) {
                cfg.deslizamiento_bps = v;
            }
            if let Some(v) = bounded(&args, "minUtilidadUsd", 0.0, 1_000_000.0) {
                cfg.min_utilidad_usd = v;
            }
            if let Some(v) = args
                .get("enfriamientoMs")
                .and_then(Value::as_i64)
                .filter(|v| (0..=3_600_000).contains(v))
            {
                cfg.enfriamiento_ms = v;
            }
            motor.actualizar_config(cfg.clone()).await;
            json!({"ok":true,"configuracion":cfg})
        }
        "update_parameters" => json!({"ok":false,"error":"Requiere Manage Server o Administrator"}),
        _ => json!({"ok":false,"error":"Tool desconocida"}),
    }
}

fn bounded(args: &Value, key: &str, min: f64, max: f64) -> Option<f64> {
    args.get(key)
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite() && (min..=max).contains(v))
}

async fn chat_fallback(
    config: &ConfigNvidia,
    messages: &[Value],
    tools: &[Value],
) -> Result<(String, Value), String> {
    let client = reqwest::Client::new();
    let mut errors = Vec::new();
    for model in &config.models {
        let response = client.post("https://integrate.api.nvidia.com/v1/chat/completions")
            .bearer_auth(&config.api_key).timeout(Duration::from_secs(45))
            .json(&json!({"model":model,"messages":messages,"tools":tools,"tool_choice":"auto","temperature":0.2,"max_tokens":900,"stream":false})).send().await;
        match response {
            Ok(response) if response.status().is_success() => {
                match response.json::<Value>().await {
                    Ok(body) if body.pointer("/choices/0/message").is_some() => {
                        return Ok((model.clone(), body["choices"][0]["message"].clone()))
                    }
                    Ok(_) => errors.push(format!("{model}: respuesta vacia")),
                    Err(error) => errors.push(format!("{model}: JSON invalido ({error})")),
                }
            }
            Ok(response) => errors.push(format!("{model}: HTTP {}", response.status())),
            Err(error) => errors.push(format!("{model}: {error}")),
        }
    }
    Err(errors.join("; "))
}

async fn completar_interaccion(application_id: &str, token: &str, content: &str) {
    if application_id.is_empty() || token.is_empty() {
        tracing::warn!(
            "no se pudo completar Discord: faltan application_id o token de interaccion"
        );
        return;
    }
    let url =
        format!("https://discord.com/api/v10/webhooks/{application_id}/{token}/messages/@original");
    match reqwest::Client::new()
        .patch(url)
        .json(&json!({"content":truncar(content),"allowed_mentions":{"parse":[]}}))
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => {
            tracing::info!("respuesta diferida de Discord completada");
        }
        Ok(response) => {
            let status = response.status();
            let detalle = response
                .text()
                .await
                .unwrap_or_else(|_| "respuesta ilegible".into());
            tracing::warn!(%status, detalle = %truncar_log(&detalle), "Discord rechazo la respuesta diferida");
        }
        Err(error) => tracing::warn!(%error, "no se pudo completar la respuesta de Discord"),
    }
}

fn truncar_log(value: &str) -> String {
    value.chars().take(500).collect()
}

fn truncar(value: &str) -> String {
    value.chars().take(1_950).collect()
}

fn env_optional(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    #[test]
    fn valida_firma_y_permisos() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let body = br#"{"type":1}"#;
        let timestamp = "1710000000";
        let mut message = timestamp.as_bytes().to_vec();
        message.extend_from_slice(body);
        let mut headers = HeaderMap::new();
        headers.insert(TIMESTAMP_HEADER, timestamp.parse().unwrap());
        headers.insert(
            SIGNATURE_HEADER,
            hex::encode(key.sign(&message).to_bytes()).parse().unwrap(),
        );
        assert!(firma_valida(&key.verifying_key(), &headers, body));
        assert!(!firma_valida(
            &key.verifying_key(),
            &headers,
            br#"{"type":2}"#
        ));
        assert!(permisos_admin("32"));
        assert!(!permisos_admin("0"));
    }
}
