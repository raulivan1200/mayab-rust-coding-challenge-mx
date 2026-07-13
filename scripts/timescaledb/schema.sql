-- Esquema TimescaleDB para la auditoría de Mayab Arbitraje BTC.
--
-- Objetivo: mismo contrato que la auditoría SQLite local, pero sobre
-- hypertables de TimescaleDB para retención y consulta temporal eficiente.
-- Esta migración es idempotente: puede ejecutarse varias veces.
--
-- Uso:
--   psql -v ON_ERROR_STOP=1 "$DATABASE_URL" -f scripts/timescaledb/schema.sql

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

-- Los hypertables no pueden imponer UNIQUE(id) sin incluir la partición
-- temporal. Esta tabla regular reclama cada identidad dentro de la misma
-- sentencia que escribe el evento, por lo que un retry o un reinicio no duplica
-- PnL ni evidencia durable.
CREATE TABLE IF NOT EXISTS audit_idempotency_keys (
    kind          TEXT NOT NULL,
    id            TEXT NOT NULL,
    claimed_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (kind, id)
);

-- Convertir en hypertables (solo si aún no lo son).
SELECT create_hypertable('operaciones', 'tiempo', if_not_exists => TRUE);
SELECT create_hypertable('eventos', 'tiempo', if_not_exists => TRUE);
SELECT create_hypertable('oportunidades', 'tiempo', if_not_exists => TRUE);
SELECT create_hypertable('auditorias', 'tiempo', if_not_exists => TRUE);
SELECT create_hypertable('rebalanceos', 'tiempo', if_not_exists => TRUE);

-- Acelera tanto consultas forenses como la reparación única de instalaciones
-- que pudieron recibir retries antes de que existiera la tabla de claves.
CREATE INDEX IF NOT EXISTS idx_operaciones_identidad ON operaciones (id, tiempo);
CREATE INDEX IF NOT EXISTS idx_eventos_identidad ON eventos (id, tiempo);
CREATE INDEX IF NOT EXISTS idx_oportunidades_identidad ON oportunidades (id, tiempo);
CREATE INDEX IF NOT EXISTS idx_auditorias_identidad ON auditorias (id, tiempo);
CREATE INDEX IF NOT EXISTS idx_rebalanceos_identidad ON rebalanceos (id, tiempo);

-- Conservar la primera observación de cada identidad. Si dos retries tienen el
-- mismo timestamp necesariamente caen en el mismo chunk, por lo que ctid sólo
-- se usa como desempate local. A partir de aquí la reclamación transaccional
-- impide que vuelvan a aparecer duplicados.
DELETE FROM operaciones AS duplicada
USING operaciones AS canonica
WHERE duplicada.id = canonica.id
  AND (
    duplicada.tiempo > canonica.tiempo
    OR (duplicada.tiempo = canonica.tiempo AND duplicada.ctid > canonica.ctid)
  );
DELETE FROM eventos AS duplicada
USING eventos AS canonica
WHERE duplicada.id = canonica.id
  AND (
    duplicada.tiempo > canonica.tiempo
    OR (duplicada.tiempo = canonica.tiempo AND duplicada.ctid > canonica.ctid)
  );
DELETE FROM oportunidades AS duplicada
USING oportunidades AS canonica
WHERE duplicada.id = canonica.id
  AND (
    duplicada.tiempo > canonica.tiempo
    OR (duplicada.tiempo = canonica.tiempo AND duplicada.ctid > canonica.ctid)
  );
DELETE FROM auditorias AS duplicada
USING auditorias AS canonica
WHERE duplicada.id = canonica.id
  AND (
    duplicada.tiempo > canonica.tiempo
    OR (duplicada.tiempo = canonica.tiempo AND duplicada.ctid > canonica.ctid)
  );
DELETE FROM rebalanceos AS duplicada
USING rebalanceos AS canonica
WHERE duplicada.id = canonica.id
  AND (
    duplicada.tiempo > canonica.tiempo
    OR (duplicada.tiempo = canonica.tiempo AND duplicada.ctid > canonica.ctid)
  );

-- Sembrar identidades de instalaciones existentes antes de aceptar escrituras
-- nuevas. DISTINCT hace segura la migración incluso si una versión previa ya
-- había recibido el mismo ID más de una vez.
INSERT INTO audit_idempotency_keys (kind, id)
SELECT 'operacion', id FROM operaciones GROUP BY id
ON CONFLICT DO NOTHING;
INSERT INTO audit_idempotency_keys (kind, id)
SELECT 'evento', id FROM eventos GROUP BY id
ON CONFLICT DO NOTHING;
INSERT INTO audit_idempotency_keys (kind, id)
SELECT 'oportunidad', id FROM oportunidades GROUP BY id
ON CONFLICT DO NOTHING;
INSERT INTO audit_idempotency_keys (kind, id)
SELECT 'auditoria', id FROM auditorias GROUP BY id
ON CONFLICT DO NOTHING;
INSERT INTO audit_idempotency_keys (kind, id)
SELECT 'rebalanceo', id FROM rebalanceos GROUP BY id
ON CONFLICT DO NOTHING;

-- Las claves sobreviven a los datos retenidos para bloquear retries tardíos.
-- Sólo se purgan después del doble de la ventana y cuando la fila ya no existe.
-- Reejecutar esta migración en mantenimiento aplica la limpieza sin tocar
-- identidades que todavía protegen evidencia auditable.
DELETE FROM audit_idempotency_keys AS llave
WHERE llave.claimed_at < NOW() - INTERVAL '180 days'
  AND CASE llave.kind
    WHEN 'operacion' THEN NOT EXISTS (SELECT 1 FROM operaciones t WHERE t.id = llave.id)
    WHEN 'evento' THEN NOT EXISTS (SELECT 1 FROM eventos t WHERE t.id = llave.id)
    WHEN 'oportunidad' THEN NOT EXISTS (SELECT 1 FROM oportunidades t WHERE t.id = llave.id)
    WHEN 'auditoria' THEN NOT EXISTS (SELECT 1 FROM auditorias t WHERE t.id = llave.id)
    WHEN 'rebalanceo' THEN NOT EXISTS (SELECT 1 FROM rebalanceos t WHERE t.id = llave.id)
    ELSE FALSE
  END;

-- Retención: conservar 90 días de auditoría por defecto.
SELECT add_retention_policy('operaciones', INTERVAL '90 days', if_not_exists => TRUE);
SELECT add_retention_policy('auditorias', INTERVAL '90 days', if_not_exists => TRUE);
SELECT add_retention_policy('eventos', INTERVAL '90 days', if_not_exists => TRUE);
SELECT add_retention_policy('oportunidades', INTERVAL '90 days', if_not_exists => TRUE);
SELECT add_retention_policy('rebalanceos', INTERVAL '90 days', if_not_exists => TRUE);

CREATE INDEX IF NOT EXISTS idx_operaciones_ruta ON operaciones (compra_en, venta_en, par, tiempo DESC);
CREATE INDEX IF NOT EXISTS idx_eventos_tipo ON eventos (tipo, tiempo DESC);
CREATE INDEX IF NOT EXISTS idx_auditorias_decision ON auditorias (decision, tiempo DESC);
CREATE INDEX IF NOT EXISTS idx_ejecuciones_tiempo ON ejecuciones (tiempo DESC);
