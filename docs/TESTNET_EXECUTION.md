# Fase 7: ejecución segura en Coinbase Exchange Sandbox

Esta superficie no forma parte del servidor público. Se compila como el binario
`mayab-testnet-executor` únicamente con `testnet-execution`; no abre puertos ni
comparte el motor, rutas Axum o configuración tolerante de la demo.

## Controles que fallan cerrados

- El único host aceptado en código es `api-public.sandbox.exchange.coinbase.com`.
- El arranque exige `TESTNET_EXECUTION_CONFIRM=COINBASE_SANDBOX_ONLY` y
  `COINBASE_SANDBOX_KEY_PERMISSIONS=view,trade` exactamente.
- Las credenciales usan nombres `COINBASE_SANDBOX_*`, distintos de producción.
- El arranque exige `TESTNET_ALLOWED_EGRESS_IP` como IP pública individual y
  registra esa identidad en el ledger. La key debe tener exactamente esa IP en
  su allowlist del proveedor; una IP privada, loopback o link-local falla cerrada.
- `TESTNET_SECRET_VERSION` debe identificar una versión fija; `latest` se rechaza
  para que cada ejecución sea atribuible y reversible.
- `TradingTransport` sólo ofrece cuentas, preflight de permisos, órdenes,
  consulta/cancelación y fills. El motor no recibe un cliente HTTP.
- La allowlist admite siete contratos de método/ruta. No existe una operación de
  depósitos, retiros, direcciones, wallets o transferencias.
- El preflight consulta perfiles y bloquea si la respuesta anuncia capacidad de
  transferencia. La declaración local View/Trade es obligatoria porque Exchange
  Sandbox no garantiza una introspección uniforme de scopes para todas las keys.
- El cliente desactiva redirects, requiere HTTPS y no registra body, headers,
  firmas, passphrase ni secretos.
- Cada orden queda limitada a 0.001 BTC y USD 25 de nocional; exceder cualquiera
  de esos topes bloquea el arranque.

## Ciclo mínimo

El ejecutor consulta perfiles y cuentas, envía una limit post-only pequeña con
`client_oid` determinista y adapta la respuesta a estados explícitos `accepted`,
`partial`, `filled`, `canceled`, `rejected`, `timeout` y `late_fill`. Un
`status=open` con `filled_size > 0` se reconoce como parcial. Si vence, captura
fills antes de cancelar, cancela, vuelve a consultar orden/fills y sólo marca
`late_fill` cuando aparecen ejecuciones nuevas después de la cancelación. Luego
reconcilia cuentas y exporta exposición final. Cada paso se escribe en un JSONL
con hash encadenado; al terminar, un lector independiente reabre y verifica toda
la cadena.

Compilación y validación sin credenciales:

```bash
cargo test --workspace --features testnet-execution
./scripts/check-testnet-safety.sh
cargo build --release --bin mayab-testnet-executor --features testnet-execution
```

Ejecución controlada (los tres secretos pueden ser archivos montados):

```bash
export TESTNET_EXECUTION_CONFIRM=COINBASE_SANDBOX_ONLY
export COINBASE_SANDBOX_HOST=api-public.sandbox.exchange.coinbase.com
export COINBASE_SANDBOX_KEY_PERMISSIONS=view,trade
export COINBASE_SANDBOX_API_KEY_FILE=/var/run/secrets/mayab/api-key
export COINBASE_SANDBOX_API_SECRET_FILE=/var/run/secrets/mayab/api-secret
export COINBASE_SANDBOX_PASSPHRASE_FILE=/var/run/secrets/mayab/passphrase
export TESTNET_PRODUCT_ID=BTC-USD TESTNET_ORDER_SIDE=buy
export TESTNET_RUN_ID=change-ticket-123
export TESTNET_ALLOWED_EGRESS_IP=34.120.10.20
export TESTNET_SECRET_VERSION=rotation-2026-07-12
export TESTNET_LIMIT_PRICE=1000.00 TESTNET_ORDER_SIZE=0.0001
export TESTNET_TIMEOUT_MS=15000 TESTNET_POLL_MS=1000
export TESTNET_LEDGER_PATH=/ledger/run.jsonl
cargo run --release --bin mayab-testnet-executor --features testnet-execution
```

El precio debe elegirse conscientemente para la prueba. `post_only` reduce la
posibilidad de ejecución inmediata, pero el sandbox puede llenarla; use capital
ficticio mínimo y confirme la exposición final del ledger.

## Cloud Run privado, Secret Manager e IP fija

Despliegue este binario como **otro Cloud Run Job**, nunca como revisión del
servicio público. Use una service account dedicada que sólo tenga
`roles/secretmanager.secretAccessor` sobre las tres versiones de secreto. Monte
cada secreto como archivo; no lo pase por `--set-env-vars`, build args o imagen.

```bash
gcloud builds submit --tag "$REGION-docker.pkg.dev/$PROJECT/$REPO/mayab-testnet:$REV" -f Dockerfile.testnet
gcloud run jobs deploy mayab-testnet-executor \
  --image "$REGION-docker.pkg.dev/$PROJECT/$REPO/mayab-testnet:$REV" \
  --region "$REGION" --service-account mayab-testnet@$PROJECT.iam.gserviceaccount.com \
  --vpc-connector mayab-testnet-egress --vpc-egress all-traffic \
  --set-secrets '/var/run/secrets/mayab/api-key=coinbase-sandbox-api-key:7,/var/run/secrets/mayab/api-secret=coinbase-sandbox-api-secret:7,/var/run/secrets/mayab/passphrase=coinbase-sandbox-passphrase:7' \
  --set-env-vars 'TESTNET_EXECUTION_CONFIRM=COINBASE_SANDBOX_ONLY,COINBASE_SANDBOX_HOST=api-public.sandbox.exchange.coinbase.com,COINBASE_SANDBOX_KEY_PERMISSIONS=view\,trade,TESTNET_ALLOWED_EGRESS_IP=34.120.10.20,TESTNET_SECRET_VERSION=sm-7,COINBASE_SANDBOX_API_KEY_FILE=/var/run/secrets/mayab/api-key,COINBASE_SANDBOX_API_SECRET_FILE=/var/run/secrets/mayab/api-secret,COINBASE_SANDBOX_PASSPHRASE_FILE=/var/run/secrets/mayab/passphrase'
```

El connector debe salir por una subred con Cloud NAT y una IP reservada. Registre
esa IP en la allowlist de la key sandbox antes de ejecutar. Verifique que no haya
otra ruta de egress y que el Job no tenga invocación pública.

La aplicación no puede modificar ni leer de forma confiable la allowlist que
protege una key de exchange. Por eso el control se divide en dos: infraestructura
fija la salida con NAT y configura la key; el binario exige la misma IP declarada,
la valida y la deja como evidencia en el ledger. Una llamada autenticada exitosa
desde el Job confirma el control del proveedor.

## Rotación y revocación

1. Cree otra key sandbox View/Trade, sin Transfer, con la misma IP permitida.
2. Agregue nuevas versiones en Secret Manager; no cambie aliases ni `latest`.
3. Despliegue una revisión del Job fijando los tres secretos al mismo número y
   cambie `TESTNET_SECRET_VERSION` a ese identificador.
4. Ejecute el ciclo mínimo y compruebe en el ledger `allowedEgressIp`,
   `secretVersion`, host y permisos.
5. Revoque la key anterior en Coinbase y deshabilite sus versiones de secretos.
6. Reejecute la revisión anterior: debe fallar autenticación. Conserve ese fallo
   sanitizado y el ledger nuevo como evidencia de revocación.
7. Ante sospecha, pause/elimine el Job, revoque primero la key, deshabilite los
   secretos y conserve ledger y audit logs. Nunca copie secretos a tickets/logs.

Referencias oficiales: [Sandbox](https://docs.cdp.coinbase.com/exchange/introduction/sandbox),
[autenticación y permisos](https://docs.cdp.coinbase.com/exchange/rest-api/authentication) y
[prácticas de seguridad](https://docs.cdp.coinbase.com/get-started/authentication/security-best-practices).
