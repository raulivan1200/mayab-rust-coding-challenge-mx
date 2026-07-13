//! Ejecutor privado y deliberadamente limitado para Coinbase Exchange Sandbox.
//! No se comparte con el motor de demo y no ofrece un cliente HTTP genérico.

use std::{env, net::IpAddr, path::PathBuf, time::Duration};

use anyhow::{bail, Context};
use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD, Engine};
use hmac::{Hmac, Mac};
use reqwest::{Client, Method};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Sha256;

use crate::ledger_audit::{independent_audit, LedgerWriter};

pub const SANDBOX_HOST: &str = "api-public.sandbox.exchange.coinbase.com";
pub const OUTBOUND_ROUTE_ALLOWLIST: &[(&str, &str)] = &[
    ("GET", "/accounts"),
    ("GET", "/profiles"),
    ("POST", "/orders"),
    ("GET", "/orders/client:{client_oid}"),
    ("GET", "/orders/{order_id}"),
    ("DELETE", "/orders/{order_id}"),
    ("GET", "/fills?order_id={order_id}"),
];

#[derive(Clone)]
pub struct TestnetConfig {
    pub api_key: String,
    pub api_secret: String,
    pub passphrase: String,
    pub product_id: String,
    pub side: String,
    pub price: String,
    pub size: String,
    pub timeout: Duration,
    pub poll_interval: Duration,
    pub ledger_path: PathBuf,
    pub run_id: String,
    pub allowed_egress_ip: IpAddr,
    pub secret_version: String,
}

impl TestnetConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        if env::var("TESTNET_EXECUTION_CONFIRM").as_deref() != Ok("COINBASE_SANDBOX_ONLY") {
            bail!("arranque bloqueado: falta confirmación exacta TESTNET_EXECUTION_CONFIRM");
        }
        if env::var("COINBASE_SANDBOX_HOST").as_deref() != Ok(SANDBOX_HOST) {
            bail!("arranque bloqueado: COINBASE_SANDBOX_HOST debe ser {SANDBOX_HOST}");
        }
        let permissions = env::var("COINBASE_SANDBOX_KEY_PERMISSIONS")
            .context("falta COINBASE_SANDBOX_KEY_PERMISSIONS=view,trade")?;
        let mut normalized: Vec<_> = permissions
            .split(',')
            .map(|v| v.trim().to_ascii_lowercase())
            .filter(|v| !v.is_empty())
            .collect();
        normalized.sort();
        normalized.dedup();
        if normalized != ["trade", "view"] {
            bail!("arranque bloqueado: la llave debe declarar únicamente View/Trade");
        }

        let side = required("TESTNET_ORDER_SIDE")?.to_ascii_lowercase();
        if side != "buy" && side != "sell" {
            bail!("TESTNET_ORDER_SIDE debe ser buy o sell");
        }
        let product_id = required("TESTNET_PRODUCT_ID")?;
        if product_id != "BTC-USD" {
            bail!("el ciclo inicial sólo permite BTC-USD");
        }
        let price = positive_decimal("TESTNET_LIMIT_PRICE")?;
        let size = positive_decimal("TESTNET_ORDER_SIZE")?;
        let price_value: rust_decimal::Decimal = price.parse()?;
        let size_value: rust_decimal::Decimal = size.parse()?;
        if size_value > rust_decimal::Decimal::new(1, 3)
            || price_value * size_value > rust_decimal::Decimal::new(25, 0)
        {
            bail!("orden bloqueada: máximo 0.001 BTC y USD 25 de nocional");
        }
        let timeout_ms = positive_u64("TESTNET_TIMEOUT_MS")?;
        let poll_ms = positive_u64("TESTNET_POLL_MS")?;
        if timeout_ms > 300_000 || poll_ms > timeout_ms {
            bail!("timeout fuera de límites seguros");
        }
        let run_id = required("TESTNET_RUN_ID")?;
        validate_id(&run_id).context("TESTNET_RUN_ID inválido")?;
        let allowed_egress_ip = required("TESTNET_ALLOWED_EGRESS_IP")?
            .parse::<IpAddr>()
            .context("TESTNET_ALLOWED_EGRESS_IP debe ser una IP pública individual")?;
        validate_public_ip(allowed_egress_ip)?;
        let secret_version = required("TESTNET_SECRET_VERSION")?;
        validate_id(&secret_version).context("TESTNET_SECRET_VERSION inválido")?;
        if secret_version.eq_ignore_ascii_case("latest") {
            bail!("TESTNET_SECRET_VERSION debe fijar una versión, no latest");
        }

        Ok(Self {
            api_key: secret("COINBASE_SANDBOX_API_KEY")?,
            api_secret: secret("COINBASE_SANDBOX_API_SECRET")?,
            passphrase: secret("COINBASE_SANDBOX_PASSPHRASE")?,
            product_id,
            side,
            price,
            size,
            timeout: Duration::from_millis(timeout_ms),
            poll_interval: Duration::from_millis(poll_ms),
            ledger_path: PathBuf::from(required("TESTNET_LEDGER_PATH")?),
            run_id,
            allowed_egress_ip,
            secret_version,
        })
    }
}

fn validate_public_ip(ip: IpAddr) -> anyhow::Result<()> {
    let prohibited = ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || match ip {
            IpAddr::V4(ip) => ip.is_private() || ip.is_link_local() || ip.is_broadcast(),
            IpAddr::V6(ip) => ip.is_unique_local() || ip.is_unicast_link_local(),
        };
    if prohibited {
        bail!("TESTNET_ALLOWED_EGRESS_IP debe ser una IP pública enrutable");
    }
    Ok(())
}

fn required(name: &str) -> anyhow::Result<String> {
    env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .with_context(|| format!("falta {name}"))
}
fn secret(name: &str) -> anyhow::Result<String> {
    let file_name = format!("{name}_FILE");
    match (env::var(name).ok(), env::var(&file_name).ok()) {
        (Some(_), Some(_)) => bail!("define sólo {name} o {file_name}"),
        (Some(value), None) if !value.trim().is_empty() => Ok(value.trim().into()),
        (None, Some(path)) => std::fs::read_to_string(&path)
            .with_context(|| format!("no se pudo leer {file_name}"))
            .map(|v| v.trim().to_string())
            .and_then(|v| {
                if v.is_empty() {
                    bail!("{file_name} está vacío")
                } else {
                    Ok(v)
                }
            }),
        _ => bail!("falta secreto sandbox {name} o {file_name}"),
    }
}
fn positive_decimal(name: &str) -> anyhow::Result<String> {
    let value = required(name)?;
    let parsed: rust_decimal::Decimal =
        value.parse().with_context(|| format!("{name} inválido"))?;
    if parsed <= rust_decimal::Decimal::ZERO {
        bail!("{name} debe ser positivo");
    }
    Ok(value)
}
fn positive_u64(name: &str) -> anyhow::Result<u64> {
    let value: u64 = required(name)?
        .parse()
        .with_context(|| format!("{name} inválido"))?;
    if value == 0 {
        bail!("{name} debe ser positivo");
    }
    Ok(value)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderRequest {
    pub client_oid: String,
    pub product_id: String,
    pub side: String,
    pub price: String,
    pub size: String,
}

#[async_trait]
pub trait TradingTransport: Send + Sync {
    async fn accounts(&self) -> anyhow::Result<Value>;
    async fn preflight_permissions(&self) -> anyhow::Result<Value>;
    async fn place_limit_order(&self, order: &OrderRequest) -> anyhow::Result<Value>;
    async fn order(&self, order_id: &str) -> anyhow::Result<Value>;
    async fn cancel_order(&self, order_id: &str) -> anyhow::Result<Value>;
    async fn fills(&self, order_id: &str) -> anyhow::Result<Value>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderLifecycle {
    Accepted,
    Partial,
    Filled,
    Canceled,
    Rejected,
    Timeout,
    LateFill,
    Open,
    Unknown,
}

fn lifecycle_from_status(status: &str) -> OrderLifecycle {
    match status.to_ascii_lowercase().as_str() {
        "open" | "pending" | "received" | "accepted" | "active" => OrderLifecycle::Accepted,
        "partial" | "partially_filled" | "partially-filled" => OrderLifecycle::Partial,
        "done" | "settled" | "filled" => OrderLifecycle::Filled,
        "canceled" | "cancelled" | "expired" => OrderLifecycle::Canceled,
        "rejected" => OrderLifecycle::Rejected,
        _ => OrderLifecycle::Unknown,
    }
}

fn lifecycle_terminal(lifecycle: OrderLifecycle) -> bool {
    matches!(
        lifecycle,
        OrderLifecycle::Filled | OrderLifecycle::Canceled | OrderLifecycle::Rejected
    )
}

fn fills_non_empty(value: &Value) -> bool {
    value
        .as_array()
        .is_some_and(|items| !items.is_empty())
}

pub struct CoinbaseSandboxTransport {
    client: Client,
    config: TestnetConfig,
}

impl CoinbaseSandboxTransport {
    pub fn new(config: TestnetConfig) -> anyhow::Result<Self> {
        let client = Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(15))
            .build()?;
        Ok(Self { client, config })
    }

    async fn allowed_request(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
    ) -> anyhow::Result<Value> {
        validate_route(&method, path)?;
        let timestamp = chrono::Utc::now().timestamp_millis() as f64 / 1000.0;
        let timestamp = format!("{timestamp:.3}");
        let body_text = body
            .map(serde_json::to_string)
            .transpose()?
            .unwrap_or_default();
        let prehash = format!("{timestamp}{}{path}{body_text}", method.as_str());
        let secret = STANDARD
            .decode(&self.config.api_secret)
            .context("secret sandbox no es base64 válido")?;
        let mut mac = Hmac::<Sha256>::new_from_slice(&secret).context("secret sandbox inválido")?;
        mac.update(prehash.as_bytes());
        let signature = STANDARD.encode(mac.finalize().into_bytes());
        let url = format!("https://{SANDBOX_HOST}{path}");
        let mut request = self
            .client
            .request(method, url)
            .header("CB-ACCESS-KEY", &self.config.api_key)
            .header("CB-ACCESS-SIGN", signature)
            .header("CB-ACCESS-TIMESTAMP", timestamp)
            .header("CB-ACCESS-PASSPHRASE", &self.config.passphrase)
            .header("Content-Type", "application/json");
        if !body_text.is_empty() {
            request = request.body(body_text);
        }
        let response = request.send().await?;
        let status = response.status();
        let bytes = response.bytes().await?;
        if !status.is_success() {
            bail!("sandbox respondió HTTP {status}: {}", safe_error(&bytes));
        }
        if bytes.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&bytes).context("respuesta sandbox no es JSON válido")
    }
}

fn safe_error(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    text.chars().filter(|c| !c.is_control()).take(240).collect()
}

#[async_trait]
impl TradingTransport for CoinbaseSandboxTransport {
    async fn accounts(&self) -> anyhow::Result<Value> {
        self.allowed_request(Method::GET, "/accounts", None).await
    }
    async fn preflight_permissions(&self) -> anyhow::Result<Value> {
        self.allowed_request(Method::GET, "/profiles", None).await
    }
    async fn place_limit_order(&self, order: &OrderRequest) -> anyhow::Result<Value> {
        let body = json!({"client_oid": order.client_oid, "product_id": order.product_id, "type":"limit", "side":order.side, "price":order.price, "size":order.size, "time_in_force":"GTC", "post_only":true});
        let mut last = None;
        for attempt in 0..3 {
            match self
                .allowed_request(Method::POST, "/orders", Some(&body))
                .await
            {
                Ok(value) => return Ok(value),
                Err(error) => {
                    last = Some(error);
                    let recovery_path = format!("/orders/client:{}", order.client_oid);
                    if let Ok(existing) = self
                        .allowed_request(Method::GET, &recovery_path, None)
                        .await
                    {
                        return Ok(existing);
                    }
                    if attempt < 2 {
                        tokio::time::sleep(Duration::from_millis(250 * (attempt + 1))).await;
                    }
                }
            }
        }
        Err(last.expect("al menos un intento"))
    }
    async fn order(&self, id: &str) -> anyhow::Result<Value> {
        validate_id(id)?;
        self.allowed_request(Method::GET, &format!("/orders/{id}"), None)
            .await
    }
    async fn cancel_order(&self, id: &str) -> anyhow::Result<Value> {
        validate_id(id)?;
        self.allowed_request(Method::DELETE, &format!("/orders/{id}"), None)
            .await
    }
    async fn fills(&self, id: &str) -> anyhow::Result<Value> {
        validate_id(id)?;
        self.allowed_request(Method::GET, &format!("/fills?order_id={id}"), None)
            .await
    }
}

fn validate_id(id: &str) -> anyhow::Result<()> {
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        bail!("id de orden inválido");
    }
    Ok(())
}

pub fn validate_route(method: &Method, path: &str) -> anyhow::Result<()> {
    let allowed = match (method.as_str(), path) {
        ("GET", "/accounts" | "/profiles") | ("POST", "/orders") => true,
        ("GET", p) if p.starts_with("/orders/client:") => validate_id(&p[15..]).is_ok(),
        ("GET" | "DELETE", p) if p.starts_with("/orders/") => validate_id(&p[8..]).is_ok(),
        ("GET", p) if p.starts_with("/fills?order_id=") => validate_id(&p[16..]).is_ok(),
        _ => false,
    };
    if !allowed {
        bail!("método/ruta saliente fuera de allowlist");
    }
    Ok(())
}

pub async fn run_cycle<T: TradingTransport>(
    transport: &T,
    config: &TestnetConfig,
) -> anyhow::Result<usize> {
    let mut ledger = LedgerWriter::create(&config.ledger_path)?;
    let permissions = transport.preflight_permissions().await?;
    if contains_transfer_capability(&permissions) {
        bail!("arranque bloqueado: capacidad Transfer detectada por preflight");
    }
    let before = transport.accounts().await?;
    ledger.append(
        "preflight",
        json!({
            "host": SANDBOX_HOST,
            "permissions":"view,trade",
            "allowedEgressIp": config.allowed_egress_ip,
            "secretVersion": config.secret_version,
            "accounts": before
        }),
    )?;

    let client_oid = deterministic_client_oid(config);
    let request = OrderRequest {
        client_oid,
        product_id: config.product_id.clone(),
        side: config.side.clone(),
        price: config.price.clone(),
        size: config.size.clone(),
    };
    let placed = transport.place_limit_order(&request).await?;
    let order_id = placed
        .get("id")
        .and_then(Value::as_str)
        .context("respuesta de orden sin id")?
        .to_string();
    validate_id(&order_id)?;
    ledger.append("order_submitted", json!({"orderId":order_id,"clientOid":request.client_oid,"productId":request.product_id,"side":request.side,"price":request.price,"size":request.size}))?;

    let deadline = tokio::time::Instant::now() + config.timeout;
    let final_order;
    loop {
        let state = transport.order(&order_id).await?;
        let status = state
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let lifecycle = lifecycle_from_status(status);
        ledger.append(
            "order_status",
            json!({"orderId":order_id,"status":status,"lifecycle":lifecycle,"terminal":lifecycle_terminal(lifecycle)}),
        )?;
        if lifecycle_terminal(lifecycle) {
            final_order = state;
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            let cancellation = transport.cancel_order(&order_id).await?;
            ledger.append(
                "timeout_cancel",
                json!({"orderId":order_id,"result":cancellation,"lifecycle":OrderLifecycle::Timeout}),
            )?;
            final_order = transport.order(&order_id).await?;
            let final_status = final_order
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let final_lifecycle = lifecycle_from_status(final_status);
            ledger.append(
                "post_timeout_status",
                json!({"orderId":order_id,"status":final_status,"lifecycle":final_lifecycle,"terminal":lifecycle_terminal(final_lifecycle)}),
            )?;
            let fills_after_cancel = transport.fills(&order_id).await?;
            if fills_non_empty(&fills_after_cancel)
                && !matches!(final_lifecycle, OrderLifecycle::Filled)
            {
                ledger.append(
                    "late_fill",
                    json!({"orderId":order_id,"fills":fills_after_cancel,"lifecycle":OrderLifecycle::LateFill}),
                )?;
            }
            break;
        }
        tokio::time::sleep(config.poll_interval).await;
    }
    let fills = transport.fills(&order_id).await?;
    let after = transport.accounts().await?;
    ledger.append("reconciliation", json!({"orderId":order_id,"order":final_order,"fills":fills,"balancesBefore":before,"balancesAfter":after}))?;
    ledger.append("final_exposure", exposure(&after))?;
    drop(ledger);
    independent_audit(&config.ledger_path)
}

fn deterministic_client_oid(config: &TestnetConfig) -> String {
    use sha2::Digest;
    let seed = format!(
        "{}:{}:{}:{}:{}",
        config.run_id, config.product_id, config.side, config.price, config.size
    );
    let hex = hex::encode(Sha256::digest(seed.as_bytes()));
    format!("mayab-{}", &hex[..24])
}

fn exposure(accounts: &Value) -> Value {
    let balances = accounts.as_array().into_iter().flatten().filter_map(|a| Some(json!({
        "currency": a.get("currency")?.as_str()?, "balance": a.get("balance")?.as_str()?, "available": a.get("available").and_then(Value::as_str)
    }))).collect::<Vec<_>>();
    json!({"accounts": balances})
}

fn contains_transfer_capability(value: &Value) -> bool {
    match value {
        Value::Object(map) => map.iter().any(|(key, value)| {
            let key = key.to_ascii_lowercase();
            ((key.contains("transfer") || key.contains("withdraw") || key.contains("deposit"))
                && value.as_bool() == Some(true))
                || contains_transfer_capability(value)
        }),
        Value::Array(values) => values.iter().any(contains_transfer_capability),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn allowlist_enumera_solo_superficie_de_trading() {
        assert_eq!(OUTBOUND_ROUTE_ALLOWLIST.len(), 7);
        for (method, route) in OUTBOUND_ROUTE_ALLOWLIST {
            let normalized = route.to_ascii_lowercase();
            for forbidden in ["deposit", "withdraw", "transfer", "wallet", "address"] {
                assert!(!normalized.contains(forbidden));
            }
            assert!(matches!(*method, "GET" | "POST" | "DELETE"));
        }
    }
    #[test]
    fn contrato_rechaza_endpoints_prohibidos() {
        for path in [
            "/withdrawals",
            "/deposits",
            "/transfers",
            "/coinbase-accounts",
            "/orders/x/settle",
        ] {
            for method in [Method::GET, Method::POST, Method::DELETE] {
                assert!(validate_route(&method, path).is_err(), "{method} {path}");
            }
        }
        assert!(validate_route(&Method::POST, "/orders").is_ok());
        assert!(validate_route(&Method::GET, "/orders/abc-123").is_ok());
    }
    #[test]
    fn host_es_exactamente_sandbox() {
        assert_eq!(SANDBOX_HOST, "api-public.sandbox.exchange.coinbase.com");
        assert!(!SANDBOX_HOST.eq_ignore_ascii_case("api.exchange.coinbase.com"));
    }
    #[test]
    fn allowlist_ip_rechaza_redes_no_publicas() {
        for ip in ["127.0.0.1", "10.0.0.1", "169.254.1.1", "::1", "fc00::1"] {
            assert!(validate_public_ip(ip.parse().unwrap()).is_err());
        }
        assert!(validate_public_ip("34.120.10.20".parse().unwrap()).is_ok());
    }
    #[test]
    fn preflight_detecta_capacidad_transfer() {
        assert!(contains_transfer_capability(&json!({"can_transfer":true})));
        assert!(!contains_transfer_capability(
            &json!({"can_transfer":false,"view":true,"trade":true})
        ));
    }
}
