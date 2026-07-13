-- Esquema TimescaleDB para la auditoría de Mayab Arbitraje BTC.
--
-- Objetivo: mismo contrato que la auditoría SQLite local, pero sobre
-- hypertables de TimescaleDB para retención y consulta temporal eficiente.
-- Esta migración es idempotente: puede ejecutarse varias veces.
--
-- Uso:
--   psql "$DATABASE_URL" -f scripts/timescaledb/schema.sql

CREATE EXTENSION IF NOT EXISTS timescaledb;

CREATE TABLE IF NOT EXISTS operaciones (
    tiempo        TIMESTAMPTZ NOT NULL,
    id            TEXT NOT NULL,
    compra_en     TEXT NOT NULL,
    venta_en      TEXT NOT NULL,
    par           TEXT NOT NULL,
    cantidad_btc  DOUBLE PRECISION NOT NULL,
    utilidad_usd  DOUBLE PRECISION NOT NULL,
    costo_usd     DOUBLE PRECISION NOT NULL,
    score         DOUBLE PRECISION,
    partial_fill  BOOLEAN DEFAULT FALSE,
    payload_json  JSONB
);

CREATE TABLE IF NOT EXISTS eventos (
    tiempo       TIMESTAMPTZ NOT NULL,
    id           TEXT NOT NULL,
    tipo         TEXT NOT NULL,
    severidad    TEXT NOT NULL,
    mensaje      TEXT,
    payload_json JSONB
);

CREATE TABLE IF NOT EXISTS oportunidades (
    tiempo        TIMESTAMPTZ NOT NULL,
    id            TEXT NOT NULL,
    ruta          TEXT NOT NULL,
    utilidad_usd  DOUBLE PRECISION NOT NULL,
    diferencial   DOUBLE PRECISION,
    payload_json  JSONB
);

CREATE TABLE IF NOT EXISTS auditorias (
    tiempo        TIMESTAMPTZ NOT NULL,
    id            TEXT NOT NULL,
    ruta          TEXT NOT NULL,
    decision      TEXT NOT NULL,
    score         DOUBLE PRECISION,
    utilidad_usd  DOUBLE PRECISION,
    razon         TEXT,
    payload_json  JSONB
);

CREATE TABLE IF NOT EXISTS rebalanceos (
    tiempo        TIMESTAMPTZ NOT NULL,
    id            TEXT NOT NULL,
    desde         TEXT,
    hacia         TEXT,
    cantidad      DOUBLE PRECISION,
    costo_usd     DOUBLE PRECISION,
    payload_json  JSONB
);

-- Estado terminal/checkpoint por id: tabla transaccional regular para que el
-- mismo execution_id sea idempotente incluso después de reiniciar el proceso.
CREATE TABLE IF NOT EXISTS ejecuciones (
    tiempo        TIMESTAMPTZ NOT NULL,
    id            TEXT PRIMARY KEY,
    escenario     TEXT NOT NULL,
    estado        TEXT NOT NULL,
    pnl_usd       TEXT NOT NULL,
    payload_json  JSONB NOT NULL
);

-- Convertir en hypertables (solo si aún no lo son).
SELECT create_hypertable('operaciones', 'tiempo', if_not_exists => TRUE);
SELECT create_hypertable('eventos', 'tiempo', if_not_exists => TRUE);
SELECT create_hypertable('oportunidades', 'tiempo', if_not_exists => TRUE);
SELECT create_hypertable('auditorias', 'tiempo', if_not_exists => TRUE);
SELECT create_hypertable('rebalanceos', 'tiempo', if_not_exists => TRUE);

-- Retención: conservar 90 días de auditoría por defecto.
SELECT add_retention_policy('operaciones', INTERVAL '90 days', if_not_exists => TRUE);
SELECT add_retention_policy('auditorias', INTERVAL '90 days', if_not_exists => TRUE);

CREATE INDEX IF NOT EXISTS idx_operaciones_ruta ON operaciones (ruta, tiempo DESC);
CREATE INDEX IF NOT EXISTS idx_eventos_tipo ON eventos (tipo, tiempo DESC);
CREATE INDEX IF NOT EXISTS idx_auditorias_decision ON auditorias (decision, tiempo DESC);
CREATE INDEX IF NOT EXISTS idx_ejecuciones_tiempo ON ejecuciones (tiempo DESC);
