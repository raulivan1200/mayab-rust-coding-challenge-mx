//! Mayab Arbitraje BTC - Librería core para tests de integración.

pub mod auditoria;
pub mod config;
pub mod discord;
pub mod estrategia;
pub mod evaluation;
pub mod ga;
pub mod http;
pub mod impacto;
pub mod ledger_audit;
pub mod mercado;
pub mod metricas;
pub mod microestructura;
pub mod motor;
pub mod ou;
pub mod persistencia;
#[cfg(feature = "timescaledb")]
pub mod persistencia_timescale;
pub mod server;
pub mod tape;
#[cfg(feature = "testnet-execution")]
pub mod testnet;
pub mod types;
pub mod version;
