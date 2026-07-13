//! Simulador forense y determinista de ejecución de arbitraje en dos piernas.
//!
//! El módulo es deliberadamente autocontenido: no envía órdenes, no comparte
//! estado con el motor live y no requiere secretos. Cada escenario pasa por la
//! misma máquina de estados, ledger decimal, deduplicación de fills y reservas.

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
};

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

const RESIDUAL_TOLERANCE_BTC: Decimal = Decimal::ZERO;

/// Los doce escenarios mínimos de la matriz forense de dos piernas.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionScenario {
    BothLegsFilled,
    Leg1FilledLeg2Failed,
    Leg1PartialLeg2Filled,
    BothLegsPartial,
    TimeoutBeforeLeg2,
    TimeoutAfterLeg2,
    DuplicateResponse,
    OutOfOrderEvent,
    RestartDuringExecution,
    IdempotentRetryAfterRestart,
    RehedgeCheaper,
    UnwindCheaper,
}

impl ExecutionScenario {
    /// Matriz mínima que una implementación de dos piernas debe reconciliar.
    /// El orden es estable para que API, exports y pruebas produzcan la misma
    /// evidencia en cada corrida.
    pub const ALL: [Self; 12] = [
        Self::BothLegsFilled,
        Self::Leg1FilledLeg2Failed,
        Self::Leg1PartialLeg2Filled,
        Self::BothLegsPartial,
        Self::TimeoutBeforeLeg2,
        Self::TimeoutAfterLeg2,
        Self::DuplicateResponse,
        Self::OutOfOrderEvent,
        Self::RestartDuringExecution,
        Self::IdempotentRetryAfterRestart,
        Self::RehedgeCheaper,
        Self::UnwindCheaper,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BothLegsFilled => "both_legs_filled",
            Self::Leg1FilledLeg2Failed => "leg1_filled_leg2_failed",
            Self::Leg1PartialLeg2Filled => "leg1_partial_leg2_filled",
            Self::BothLegsPartial => "both_legs_partial",
            Self::TimeoutBeforeLeg2 => "timeout_before_leg2",
            Self::TimeoutAfterLeg2 => "timeout_after_leg2",
            Self::DuplicateResponse => "duplicate_response",
            Self::OutOfOrderEvent => "out_of_order_event",
            Self::RestartDuringExecution => "restart_during_execution",
            Self::IdempotentRetryAfterRestart => "idempotent_retry_after_restart",
            Self::RehedgeCheaper => "rehedge_cheaper",
            Self::UnwindCheaper => "unwind_cheaper",
        }
    }
}

/// Estados persistibles de una ejecución de dos piernas.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ExecutionState {
    Detected,
    Reserved,
    Leg1Submitted,
    Leg1Partial,
    Leg1Filled,
    Leg2Submitted,
    Leg2Partial,
    Leg2Filled,
    Leg2Rejected,
    Leg2TimedOut,
    RecoverySelected,
    Reconciled,
}

impl ExecutionState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Detected => "DETECTED",
            Self::Reserved => "RESERVED",
            Self::Leg1Submitted => "LEG1_SUBMITTED",
            Self::Leg1Partial => "LEG1_PARTIAL",
            Self::Leg1Filled => "LEG1_FILLED",
            Self::Leg2Submitted => "LEG2_SUBMITTED",
            Self::Leg2Partial => "LEG2_PARTIAL",
            Self::Leg2Filled => "LEG2_FILLED",
            Self::Leg2Rejected => "LEG2_REJECTED",
            Self::Leg2TimedOut => "LEG2_TIMED_OUT",
            Self::RecoverySelected => "RECOVERY_SELECTED",
            Self::Reconciled => "RECONCILED",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionLeg {
    Leg1,
    Leg2,
    Recovery,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderSide {
    Buy,
    Sell,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryAction {
    Rehedge,
    Unwind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LedgerEntryKind {
    Reservation,
    Fill,
    ReservationRelease,
    Reconciliation,
}

/// Balance total inicial de un venue. Las reservas comienzan en cero.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitialWallet {
    pub venue: String,
    pub usd: Decimal,
    pub btc: Decimal,
}

/// Snapshot total y reservado. El disponible es `total - reservado`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WalletSnapshot {
    pub venue: String,
    pub usd: Decimal,
    pub btc: Decimal,
    pub reserved_usd: Decimal,
    pub reserved_btc: Decimal,
}

impl From<InitialWallet> for WalletSnapshot {
    fn from(value: InitialWallet) -> Self {
        Self {
            venue: value.venue,
            usd: value.usd,
            btc: value.btc,
            reserved_usd: Decimal::ZERO,
            reserved_btc: Decimal::ZERO,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionPrices {
    pub leg1_buy_price_usd: Decimal,
    pub leg2_sell_price_usd: Decimal,
    pub rehedge_price_usd: Decimal,
    pub unwind_price_usd: Decimal,
}

/// Costos fijos del tamaño solicitado. Los costos de piernas se prorratean
/// cuando un escenario llena sólo parte de la orden.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionCosts {
    pub leg1_fee_usd: Decimal,
    pub leg2_fee_usd: Decimal,
    pub rehedge_cost_usd: Decimal,
    pub unwind_cost_usd: Decimal,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionRequest {
    pub execution_id: String,
    pub scenario: ExecutionScenario,
    pub buy_wallet: InitialWallet,
    pub sell_wallet: InitialWallet,
    pub quantity_btc: Decimal,
    pub prices: ExecutionPrices,
    pub costs: ExecutionCosts,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionFill {
    pub fill_id: String,
    pub exchange_sequence: u64,
    pub leg: ExecutionLeg,
    pub venue: String,
    pub side: OrderSide,
    pub quantity_btc: Decimal,
    pub price_usd: Decimal,
    pub fee_usd: Decimal,
    pub reservation_consumed_usd: Decimal,
    pub reservation_consumed_btc: Decimal,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WalletDelta {
    pub venue: String,
    pub usd: Decimal,
    pub btc: Decimal,
    pub reserved_usd: Decimal,
    pub reserved_btc: Decimal,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WalletLedgerEntry {
    pub sequence: u64,
    pub kind: LedgerEntryKind,
    pub reference: String,
    pub deltas: Vec<WalletDelta>,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionTransition {
    pub sequence: u64,
    pub from: Option<ExecutionState>,
    pub to: ExecutionState,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionInvariants {
    pub terminal_reconciled: bool,
    pub residual_within_tolerance: bool,
    pub btc_conserved: bool,
    pub wallet_equation: bool,
    pub ledger_reconciled: bool,
    pub pnl_matches_ledger: bool,
    pub fills_unique: bool,
    pub fill_sequences_monotonic: bool,
    pub reservations_released_once: bool,
    pub no_negative_balances: bool,
    pub all_passed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionReport {
    pub execution_id: String,
    pub scenario: ExecutionScenario,
    pub state: ExecutionState,
    pub wallets_before: Vec<WalletSnapshot>,
    pub wallets_after: Vec<WalletSnapshot>,
    pub fills: Vec<ExecutionFill>,
    pub ledger: Vec<WalletLedgerEntry>,
    pub transitions: Vec<ExecutionTransition>,
    pub pnl_usd: Decimal,
    pub ledger_pnl_usd: Decimal,
    pub residual_before_recovery_btc: Decimal,
    pub residual_btc: Decimal,
    pub selected_recovery: Option<RecoveryAction>,
    pub invariants: ExecutionInvariants,
    pub duplicates_ignored: u32,
    pub out_of_order_ignored: u32,
    pub reservation_release_count: u32,
    pub reservation_retries_ignored: u32,
    pub restart_performed: bool,
    pub retry_performed: bool,
    pub state_rehydrated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum ExecutionError {
    InvalidRequest {
        message: String,
    },
    InvalidTransition {
        from: Option<ExecutionState>,
        to: ExecutionState,
    },
    InsufficientBalance {
        venue: String,
        asset: String,
        required: Decimal,
        available: Decimal,
    },
    UnreconciledResidual {
        residual_btc: Decimal,
    },
    NoFeasibleRecovery {
        residual_btc: Decimal,
    },
    Checkpoint {
        message: String,
    },
    Internal {
        message: String,
    },
}

impl fmt::Display for ExecutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRequest { message } => write!(formatter, "solicitud inválida: {message}"),
            Self::InvalidTransition { from, to } => {
                write!(formatter, "transición inválida: {from:?} -> {to:?}")
            }
            Self::InsufficientBalance {
                venue,
                asset,
                required,
                available,
            } => write!(
                formatter,
                "saldo insuficiente en {venue}: requiere {required} {asset}, disponible {available}"
            ),
            Self::UnreconciledResidual { residual_btc } => {
                write!(
                    formatter,
                    "exposición residual no conciliada: {residual_btc} BTC"
                )
            }
            Self::NoFeasibleRecovery { residual_btc } => {
                write!(
                    formatter,
                    "no existe recuperación factible para {residual_btc} BTC"
                )
            }
            Self::Checkpoint { message } => write!(formatter, "checkpoint inválido: {message}"),
            Self::Internal { message } => write!(formatter, "error interno: {message}"),
        }
    }
}

impl Error for ExecutionError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum ReservationUse {
    None,
    Usd,
    Btc,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Simulator {
    request: ExecutionRequest,
    wallets_before: BTreeMap<String, WalletSnapshot>,
    wallets: BTreeMap<String, WalletSnapshot>,
    state: Option<ExecutionState>,
    transitions: Vec<ExecutionTransition>,
    fills: Vec<ExecutionFill>,
    ledger: Vec<WalletLedgerEntry>,
    seen_fill_ids: BTreeSet<String>,
    last_exchange_sequence: u64,
    reservation_created: bool,
    reservations_released: bool,
    reservation_release_count: u32,
    reservation_retries_ignored: u32,
    duplicates_ignored: u32,
    out_of_order_ignored: u32,
    selected_recovery: Option<RecoveryAction>,
    residual_before_recovery_btc: Decimal,
    restart_performed: bool,
    retry_performed: bool,
    state_rehydrated: bool,
}

struct FillDraft<'a> {
    id_suffix: &'a str,
    exchange_sequence: u64,
    leg: ExecutionLeg,
    venue: String,
    side: OrderSide,
    quantity_btc: Decimal,
    price_usd: Decimal,
    fee_usd: Decimal,
}

/// Ejecuta uno de los doce escenarios sobre wallets simuladas y devuelve toda
/// la evidencia necesaria para recalcular balances, P&L y exposición residual.
pub fn simulate(request: ExecutionRequest) -> Result<ExecutionReport, ExecutionError> {
    validate_request(&request)?;
    let scenario = request.scenario;
    let mut simulator = Simulator::new(request)?;
    simulator.transition(ExecutionState::Detected, "oportunidad detectada")?;
    simulator.reserve()?;

    match scenario {
        ExecutionScenario::BothLegsFilled => {
            simulator.fill_leg1(simulator.request.quantity_btc)?;
            simulator.fill_leg2(simulator.request.quantity_btc)?;
        }
        ExecutionScenario::Leg1FilledLeg2Failed => {
            simulator.fill_leg1(simulator.request.quantity_btc)?;
            simulator.reject_leg2("segunda pierna rechazada")?;
            simulator.recover()?;
        }
        ExecutionScenario::Leg1PartialLeg2Filled => {
            simulator.fill_leg1(simulator.request.quantity_btc / Decimal::TWO)?;
            simulator.fill_leg2(simulator.request.quantity_btc)?;
            simulator.recover()?;
        }
        ExecutionScenario::BothLegsPartial => {
            let partial = simulator.request.quantity_btc / Decimal::TWO;
            simulator.fill_leg1(partial)?;
            simulator.fill_leg2(partial)?;
        }
        ExecutionScenario::TimeoutBeforeLeg2 => {
            simulator.fill_leg1(simulator.request.quantity_btc)?;
            simulator.recover_with_detail("timeout antes de enviar la segunda pierna")?;
        }
        ExecutionScenario::TimeoutAfterLeg2 => {
            simulator.fill_leg1(simulator.request.quantity_btc)?;
            simulator.submit_leg2()?;
            simulator.transition(
                ExecutionState::Leg2TimedOut,
                "timeout después de enviar la segunda pierna",
            )?;
            simulator.recover()?;
        }
        ExecutionScenario::DuplicateResponse => {
            let leg1 = simulator.fill_leg1(simulator.request.quantity_btc)?;
            let _ = simulator.apply_fill(leg1, ReservationUse::Usd)?;
            simulator.fill_leg2(simulator.request.quantity_btc)?;
        }
        ExecutionScenario::OutOfOrderEvent => {
            simulator.submit_leg1()?;
            let invalid = simulator.make_fill(FillDraft {
                id_suffix: "leg1-out-of-order",
                exchange_sequence: 2,
                leg: ExecutionLeg::Leg1,
                venue: simulator.buy_venue(),
                side: OrderSide::Buy,
                quantity_btc: simulator.request.quantity_btc,
                price_usd: simulator.request.prices.leg1_buy_price_usd,
                fee_usd: simulator.request.costs.leg1_fee_usd,
            });
            let _ = simulator.apply_fill(invalid, ReservationUse::Usd)?;
            simulator.complete_leg1(simulator.request.quantity_btc)?;
            simulator.fill_leg2(simulator.request.quantity_btc)?;
        }
        ExecutionScenario::RestartDuringExecution => {
            simulator.fill_leg1(simulator.request.quantity_btc)?;
            simulator = simulator.restart()?;
            simulator.fill_leg2(simulator.request.quantity_btc)?;
        }
        ExecutionScenario::IdempotentRetryAfterRestart => {
            let leg1 = simulator.fill_leg1(simulator.request.quantity_btc)?;
            simulator = simulator.restart()?;
            simulator.retry_performed = true;
            simulator.reserve()?;
            let _ = simulator.apply_fill(leg1, ReservationUse::Usd)?;
            simulator.fill_leg2(simulator.request.quantity_btc)?;
        }
        ExecutionScenario::RehedgeCheaper | ExecutionScenario::UnwindCheaper => {
            simulator.fill_leg1(simulator.request.quantity_btc)?;
            simulator.reject_leg2("segunda pierna rechazada para comparar recuperación")?;
            simulator.recover()?;
        }
    }

    simulator.reconcile()
}

/// Ejecuta los doce casos contra wallets y precios fijos. Esta evidencia es
/// pura y reproducible: no lee mercado live, no persiste y no modifica el
/// estado compartido del motor.
pub fn standard_matrix() -> Result<Vec<ExecutionReport>, ExecutionError> {
    ExecutionScenario::ALL
        .into_iter()
        .map(|scenario| {
            let mut request = standard_request(scenario);
            if scenario == ExecutionScenario::RehedgeCheaper {
                request.prices.rehedge_price_usd = Decimal::new(50_050, 0);
                request.prices.unwind_price_usd = Decimal::new(49_950, 0);
                request.costs.rehedge_cost_usd = Decimal::ONE;
                request.costs.unwind_cost_usd = Decimal::TEN;
            } else if scenario == ExecutionScenario::UnwindCheaper {
                request.prices.rehedge_price_usd = Decimal::new(49_950, 0);
                request.prices.unwind_price_usd = Decimal::new(50_050, 0);
                request.costs.rehedge_cost_usd = Decimal::TEN;
                request.costs.unwind_cost_usd = Decimal::ONE;
            }
            simulate(request)
        })
        .collect()
}

fn standard_request(scenario: ExecutionScenario) -> ExecutionRequest {
    ExecutionRequest {
        execution_id: format!("forensic-{}", scenario.as_str()),
        scenario,
        buy_wallet: InitialWallet {
            venue: "BUY".to_string(),
            usd: Decimal::new(200_000, 0),
            btc: Decimal::ONE,
        },
        sell_wallet: InitialWallet {
            venue: "SELL".to_string(),
            usd: Decimal::new(100_000, 0),
            btc: Decimal::TWO,
        },
        quantity_btc: Decimal::ONE,
        prices: ExecutionPrices {
            leg1_buy_price_usd: Decimal::new(50_000, 0),
            leg2_sell_price_usd: Decimal::new(50_100, 0),
            rehedge_price_usd: Decimal::new(50_020, 0),
            unwind_price_usd: Decimal::new(49_980, 0),
        },
        costs: ExecutionCosts {
            leg1_fee_usd: Decimal::new(5, 0),
            leg2_fee_usd: Decimal::new(5, 0),
            rehedge_cost_usd: Decimal::new(6, 0),
            unwind_cost_usd: Decimal::new(7, 0),
        },
    }
}

fn validate_request(request: &ExecutionRequest) -> Result<(), ExecutionError> {
    if request.execution_id.trim().is_empty() {
        return Err(ExecutionError::InvalidRequest {
            message: "execution_id vacío".to_string(),
        });
    }
    if request.buy_wallet.venue.trim().is_empty()
        || request.sell_wallet.venue.trim().is_empty()
        || request.buy_wallet.venue == request.sell_wallet.venue
    {
        return Err(ExecutionError::InvalidRequest {
            message: "se requieren dos venues distintos".to_string(),
        });
    }
    if request.quantity_btc <= Decimal::ZERO {
        return Err(ExecutionError::InvalidRequest {
            message: "quantity_btc debe ser positiva".to_string(),
        });
    }
    let prices = [
        request.prices.leg1_buy_price_usd,
        request.prices.leg2_sell_price_usd,
        request.prices.rehedge_price_usd,
        request.prices.unwind_price_usd,
    ];
    if prices.iter().any(|price| *price <= Decimal::ZERO) {
        return Err(ExecutionError::InvalidRequest {
            message: "todos los precios deben ser positivos".to_string(),
        });
    }
    let non_negative = [
        request.buy_wallet.usd,
        request.buy_wallet.btc,
        request.sell_wallet.usd,
        request.sell_wallet.btc,
        request.costs.leg1_fee_usd,
        request.costs.leg2_fee_usd,
        request.costs.rehedge_cost_usd,
        request.costs.unwind_cost_usd,
    ];
    if non_negative.iter().any(|value| *value < Decimal::ZERO) {
        return Err(ExecutionError::InvalidRequest {
            message: "balances y costos no pueden ser negativos".to_string(),
        });
    }
    Ok(())
}

impl Simulator {
    fn new(request: ExecutionRequest) -> Result<Self, ExecutionError> {
        let buy = WalletSnapshot::from(request.buy_wallet.clone());
        let sell = WalletSnapshot::from(request.sell_wallet.clone());
        let mut wallets = BTreeMap::new();
        if wallets.insert(buy.venue.clone(), buy).is_some()
            || wallets.insert(sell.venue.clone(), sell).is_some()
        {
            return Err(ExecutionError::InvalidRequest {
                message: "venues duplicados".to_string(),
            });
        }
        Ok(Self {
            request,
            wallets_before: wallets.clone(),
            wallets,
            state: None,
            transitions: Vec::new(),
            fills: Vec::new(),
            ledger: Vec::new(),
            seen_fill_ids: BTreeSet::new(),
            last_exchange_sequence: 0,
            reservation_created: false,
            reservations_released: false,
            reservation_release_count: 0,
            reservation_retries_ignored: 0,
            duplicates_ignored: 0,
            out_of_order_ignored: 0,
            selected_recovery: None,
            residual_before_recovery_btc: Decimal::ZERO,
            restart_performed: false,
            retry_performed: false,
            state_rehydrated: false,
        })
    }

    fn buy_venue(&self) -> String {
        self.request.buy_wallet.venue.clone()
    }

    fn sell_venue(&self) -> String {
        self.request.sell_wallet.venue.clone()
    }

    fn transition(
        &mut self,
        to: ExecutionState,
        detail: impl Into<String>,
    ) -> Result<(), ExecutionError> {
        if !allowed_transition(self.state, to) {
            return Err(ExecutionError::InvalidTransition {
                from: self.state,
                to,
            });
        }
        let sequence = self.transitions.len() as u64 + 1;
        self.transitions.push(ExecutionTransition {
            sequence,
            from: self.state,
            to,
            detail: detail.into(),
        });
        self.state = Some(to);
        Ok(())
    }

    fn reserve(&mut self) -> Result<(), ExecutionError> {
        if self.reservation_created {
            self.reservation_retries_ignored += 1;
            return Ok(());
        }
        let usd = self.request.quantity_btc * self.request.prices.leg1_buy_price_usd
            + self.request.costs.leg1_fee_usd;
        let btc = self.request.quantity_btc;
        let buy_venue = self.buy_venue();
        let sell_venue = self.sell_venue();
        let buy_available = self.available_usd(&buy_venue)?;
        if buy_available < usd {
            return Err(ExecutionError::InsufficientBalance {
                venue: buy_venue,
                asset: "USD".to_string(),
                required: usd,
                available: buy_available,
            });
        }
        let sell_available = self.available_btc(&sell_venue)?;
        if sell_available < btc {
            return Err(ExecutionError::InsufficientBalance {
                venue: sell_venue,
                asset: "BTC".to_string(),
                required: btc,
                available: sell_available,
            });
        }
        self.wallet_mut(&buy_venue)?.reserved_usd += usd;
        self.wallet_mut(&sell_venue)?.reserved_btc += btc;
        self.append_ledger(
            LedgerEntryKind::Reservation,
            self.request.execution_id.clone(),
            vec![
                wallet_delta(&buy_venue, Decimal::ZERO, Decimal::ZERO, usd, Decimal::ZERO),
                wallet_delta(
                    &sell_venue,
                    Decimal::ZERO,
                    Decimal::ZERO,
                    Decimal::ZERO,
                    btc,
                ),
            ],
            "USD de compra y BTC de venta reservados",
        );
        self.reservation_created = true;
        self.transition(ExecutionState::Reserved, "capital reservado")
    }

    fn submit_leg1(&mut self) -> Result<(), ExecutionError> {
        self.transition(ExecutionState::Leg1Submitted, "primera pierna enviada")
    }

    fn fill_leg1(&mut self, quantity: Decimal) -> Result<ExecutionFill, ExecutionError> {
        self.submit_leg1()?;
        self.complete_leg1(quantity)
    }

    fn complete_leg1(&mut self, quantity: Decimal) -> Result<ExecutionFill, ExecutionError> {
        let fee = prorated_fee(
            self.request.costs.leg1_fee_usd,
            quantity,
            self.request.quantity_btc,
        );
        let fill = self.make_fill(FillDraft {
            id_suffix: "leg1-fill",
            exchange_sequence: self.last_exchange_sequence + 1,
            leg: ExecutionLeg::Leg1,
            venue: self.buy_venue(),
            side: OrderSide::Buy,
            quantity_btc: quantity,
            price_usd: self.request.prices.leg1_buy_price_usd,
            fee_usd: fee,
        });
        let accepted = self.apply_fill(fill.clone(), ReservationUse::Usd)?;
        if !accepted {
            return Err(ExecutionError::Internal {
                message: "fill válido de primera pierna fue ignorado".to_string(),
            });
        }
        let next = if quantity == self.request.quantity_btc {
            ExecutionState::Leg1Filled
        } else {
            ExecutionState::Leg1Partial
        };
        self.transition(next, "fill de primera pierna aplicado exactamente una vez")?;
        Ok(fill)
    }

    fn submit_leg2(&mut self) -> Result<(), ExecutionError> {
        self.transition(ExecutionState::Leg2Submitted, "segunda pierna enviada")
    }

    fn fill_leg2(&mut self, quantity: Decimal) -> Result<ExecutionFill, ExecutionError> {
        self.submit_leg2()?;
        let fee = prorated_fee(
            self.request.costs.leg2_fee_usd,
            quantity,
            self.request.quantity_btc,
        );
        let fill = self.make_fill(FillDraft {
            id_suffix: "leg2-fill",
            exchange_sequence: self.last_exchange_sequence + 1,
            leg: ExecutionLeg::Leg2,
            venue: self.sell_venue(),
            side: OrderSide::Sell,
            quantity_btc: quantity,
            price_usd: self.request.prices.leg2_sell_price_usd,
            fee_usd: fee,
        });
        let accepted = self.apply_fill(fill.clone(), ReservationUse::Btc)?;
        if !accepted {
            return Err(ExecutionError::Internal {
                message: "fill válido de segunda pierna fue ignorado".to_string(),
            });
        }
        let next = if quantity == self.request.quantity_btc {
            ExecutionState::Leg2Filled
        } else {
            ExecutionState::Leg2Partial
        };
        self.transition(next, "fill de segunda pierna aplicado exactamente una vez")?;
        Ok(fill)
    }

    fn reject_leg2(&mut self, detail: &str) -> Result<(), ExecutionError> {
        self.submit_leg2()?;
        self.transition(ExecutionState::Leg2Rejected, detail)
    }

    fn make_fill(&self, draft: FillDraft<'_>) -> ExecutionFill {
        ExecutionFill {
            fill_id: format!("{}-{}", self.request.execution_id, draft.id_suffix),
            exchange_sequence: draft.exchange_sequence,
            leg: draft.leg,
            venue: draft.venue,
            side: draft.side,
            quantity_btc: draft.quantity_btc,
            price_usd: draft.price_usd,
            fee_usd: draft.fee_usd,
            reservation_consumed_usd: Decimal::ZERO,
            reservation_consumed_btc: Decimal::ZERO,
        }
    }

    fn apply_fill(
        &mut self,
        mut fill: ExecutionFill,
        reservation_use: ReservationUse,
    ) -> Result<bool, ExecutionError> {
        if self.seen_fill_ids.contains(&fill.fill_id) {
            self.duplicates_ignored += 1;
            return Ok(false);
        }
        if fill.exchange_sequence != self.last_exchange_sequence + 1 {
            self.out_of_order_ignored += 1;
            return Ok(false);
        }
        if fill.quantity_btc <= Decimal::ZERO
            || fill.price_usd <= Decimal::ZERO
            || fill.fee_usd < Decimal::ZERO
        {
            return Err(ExecutionError::InvalidRequest {
                message: "fill con cantidad, precio o fee inválido".to_string(),
            });
        }

        let notional = fill.quantity_btc * fill.price_usd;
        let wallet = self.wallet_mut(&fill.venue)?;
        let (usd_delta, btc_delta, reserved_usd_delta, reserved_btc_delta) = match fill.side {
            OrderSide::Buy => {
                let required = notional + fill.fee_usd;
                let consumed = if reservation_use == ReservationUse::Usd {
                    wallet.reserved_usd.min(required)
                } else {
                    Decimal::ZERO
                };
                let available = wallet.usd - wallet.reserved_usd + consumed;
                if available < required {
                    return Err(ExecutionError::InsufficientBalance {
                        venue: fill.venue.clone(),
                        asset: "USD".to_string(),
                        required,
                        available,
                    });
                }
                wallet.usd -= required;
                wallet.btc += fill.quantity_btc;
                wallet.reserved_usd -= consumed;
                fill.reservation_consumed_usd = consumed;
                (-required, fill.quantity_btc, -consumed, Decimal::ZERO)
            }
            OrderSide::Sell => {
                let consumed = if reservation_use == ReservationUse::Btc {
                    wallet.reserved_btc.min(fill.quantity_btc)
                } else {
                    Decimal::ZERO
                };
                let available = wallet.btc - wallet.reserved_btc + consumed;
                if available < fill.quantity_btc {
                    return Err(ExecutionError::InsufficientBalance {
                        venue: fill.venue.clone(),
                        asset: "BTC".to_string(),
                        required: fill.quantity_btc,
                        available,
                    });
                }
                let proceeds = notional - fill.fee_usd;
                wallet.usd += proceeds;
                wallet.btc -= fill.quantity_btc;
                wallet.reserved_btc -= consumed;
                fill.reservation_consumed_btc = consumed;
                (proceeds, -fill.quantity_btc, Decimal::ZERO, -consumed)
            }
        };

        self.last_exchange_sequence = fill.exchange_sequence;
        self.seen_fill_ids.insert(fill.fill_id.clone());
        self.append_ledger(
            LedgerEntryKind::Fill,
            fill.fill_id.clone(),
            vec![wallet_delta(
                &fill.venue,
                usd_delta,
                btc_delta,
                reserved_usd_delta,
                reserved_btc_delta,
            )],
            format!("{:?} {:?} aplicado", fill.leg, fill.side),
        );
        self.fills.push(fill);
        Ok(true)
    }

    fn recover(&mut self) -> Result<(), ExecutionError> {
        self.recover_with_detail("recuperación seleccionada por menor costo factible")
    }

    fn recover_with_detail(&mut self, detail: &str) -> Result<(), ExecutionError> {
        let residual = self.residual_btc();
        if residual == Decimal::ZERO {
            return Ok(());
        }
        self.residual_before_recovery_btc = residual;
        let action = self.select_recovery(residual)?;
        self.selected_recovery = Some(action);
        self.transition(ExecutionState::RecoverySelected, detail)?;
        self.apply_recovery(action, residual)
    }

    fn select_recovery(&self, residual: Decimal) -> Result<RecoveryAction, ExecutionError> {
        let mut feasible = Vec::new();
        if self.recovery_feasible(RecoveryAction::Rehedge, residual)? {
            feasible.push((
                RecoveryAction::Rehedge,
                self.recovery_cash_cost(RecoveryAction::Rehedge, residual),
            ));
        }
        if self.recovery_feasible(RecoveryAction::Unwind, residual)? {
            feasible.push((
                RecoveryAction::Unwind,
                self.recovery_cash_cost(RecoveryAction::Unwind, residual),
            ));
        }
        feasible
            .into_iter()
            .min_by(|left, right| left.1.cmp(&right.1).then(left.0.cmp(&right.0)))
            .map(|candidate| candidate.0)
            .ok_or(ExecutionError::NoFeasibleRecovery {
                residual_btc: residual,
            })
    }

    /// Impacto de caja all-in comparable entre recuperaciones factibles. Para
    /// vender, más proceeds netos producen un costo menor (más negativo); para
    /// comprar, gana el menor desembolso incluyendo fee.
    fn recovery_cash_cost(&self, action: RecoveryAction, residual: Decimal) -> Decimal {
        let quantity = residual.abs();
        let (_, price, fee, _) = self.recovery_terms(action, residual);
        let notional = quantity * price;
        if residual > Decimal::ZERO {
            -(notional - fee)
        } else {
            notional + fee
        }
    }

    fn recovery_feasible(
        &self,
        action: RecoveryAction,
        residual: Decimal,
    ) -> Result<bool, ExecutionError> {
        let quantity = residual.abs();
        let (venue, price, cost, reservation_use) = self.recovery_terms(action, residual);
        let wallet = self.wallet(&venue)?;
        if residual > Decimal::ZERO {
            let reservation = if reservation_use == ReservationUse::Btc {
                wallet.reserved_btc
            } else {
                Decimal::ZERO
            };
            Ok(wallet.btc - wallet.reserved_btc + reservation >= quantity)
        } else {
            let required = quantity * price + cost;
            let reservation = if reservation_use == ReservationUse::Usd {
                wallet.reserved_usd
            } else {
                Decimal::ZERO
            };
            Ok(wallet.usd - wallet.reserved_usd + reservation >= required)
        }
    }

    fn recovery_terms(
        &self,
        action: RecoveryAction,
        residual: Decimal,
    ) -> (String, Decimal, Decimal, ReservationUse) {
        match (action, residual > Decimal::ZERO) {
            (RecoveryAction::Rehedge, true) => (
                self.sell_venue(),
                self.request.prices.rehedge_price_usd,
                self.request.costs.rehedge_cost_usd,
                ReservationUse::Btc,
            ),
            (RecoveryAction::Rehedge, false) => (
                self.buy_venue(),
                self.request.prices.rehedge_price_usd,
                self.request.costs.rehedge_cost_usd,
                ReservationUse::Usd,
            ),
            (RecoveryAction::Unwind, true) => (
                self.buy_venue(),
                self.request.prices.unwind_price_usd,
                self.request.costs.unwind_cost_usd,
                ReservationUse::None,
            ),
            (RecoveryAction::Unwind, false) => (
                self.sell_venue(),
                self.request.prices.unwind_price_usd,
                self.request.costs.unwind_cost_usd,
                ReservationUse::None,
            ),
        }
    }

    fn apply_recovery(
        &mut self,
        action: RecoveryAction,
        residual: Decimal,
    ) -> Result<(), ExecutionError> {
        let (venue, price, cost, reservation_use) = self.recovery_terms(action, residual);
        let side = if residual > Decimal::ZERO {
            OrderSide::Sell
        } else {
            OrderSide::Buy
        };
        let suffix = match action {
            RecoveryAction::Rehedge => "rehedge-fill",
            RecoveryAction::Unwind => "unwind-fill",
        };
        let fill = self.make_fill(FillDraft {
            id_suffix: suffix,
            exchange_sequence: self.last_exchange_sequence + 1,
            leg: ExecutionLeg::Recovery,
            venue,
            side,
            quantity_btc: residual.abs(),
            price_usd: price,
            fee_usd: cost,
        });
        if !self.apply_fill(fill, reservation_use)? {
            return Err(ExecutionError::Internal {
                message: "fill de recuperación fue ignorado".to_string(),
            });
        }
        Ok(())
    }

    fn residual_btc(&self) -> Decimal {
        self.fills.iter().fold(Decimal::ZERO, |residual, fill| {
            residual
                + match fill.side {
                    OrderSide::Buy => fill.quantity_btc,
                    OrderSide::Sell => -fill.quantity_btc,
                }
        })
    }

    fn release_reservations(&mut self) -> Result<(), ExecutionError> {
        if self.reservations_released {
            return Ok(());
        }
        let buy_venue = self.buy_venue();
        let sell_venue = self.sell_venue();
        let buy_usd = self.wallet(&buy_venue)?.reserved_usd;
        let buy_btc = self.wallet(&buy_venue)?.reserved_btc;
        let sell_usd = self.wallet(&sell_venue)?.reserved_usd;
        let sell_btc = self.wallet(&sell_venue)?.reserved_btc;
        {
            let buy = self.wallet_mut(&buy_venue)?;
            buy.reserved_usd = Decimal::ZERO;
            buy.reserved_btc = Decimal::ZERO;
        }
        {
            let sell = self.wallet_mut(&sell_venue)?;
            sell.reserved_usd = Decimal::ZERO;
            sell.reserved_btc = Decimal::ZERO;
        }
        self.append_ledger(
            LedgerEntryKind::ReservationRelease,
            self.request.execution_id.clone(),
            vec![
                wallet_delta(&buy_venue, Decimal::ZERO, Decimal::ZERO, -buy_usd, -buy_btc),
                wallet_delta(
                    &sell_venue,
                    Decimal::ZERO,
                    Decimal::ZERO,
                    -sell_usd,
                    -sell_btc,
                ),
            ],
            "reservas remanentes liberadas en un único batch",
        );
        self.reservations_released = true;
        self.reservation_release_count += 1;
        Ok(())
    }

    fn reconcile(mut self) -> Result<ExecutionReport, ExecutionError> {
        let residual = self.residual_btc();
        if residual.abs() > RESIDUAL_TOLERANCE_BTC {
            return Err(ExecutionError::UnreconciledResidual {
                residual_btc: residual,
            });
        }
        self.release_reservations()?;
        self.transition(
            ExecutionState::Reconciled,
            "wallets, fills, reservas y ledger conciliados",
        )?;
        self.append_ledger(
            LedgerEntryKind::Reconciliation,
            self.request.execution_id.clone(),
            Vec::new(),
            "snapshot terminal reconciliado",
        );
        self.into_report()
    }

    fn restart(mut self) -> Result<Self, ExecutionError> {
        let bytes = serde_json::to_vec(&self).map_err(|error| ExecutionError::Checkpoint {
            message: error.to_string(),
        })?;
        let mut restored: Self =
            serde_json::from_slice(&bytes).map_err(|error| ExecutionError::Checkpoint {
                message: error.to_string(),
            })?;
        restored.restart_performed = true;
        restored.state_rehydrated = true;
        self.state = None;
        Ok(restored)
    }

    fn into_report(self) -> Result<ExecutionReport, ExecutionError> {
        let state = self.state.ok_or_else(|| ExecutionError::Internal {
            message: "ejecución sin estado terminal".to_string(),
        })?;
        let wallets_before = snapshots(&self.wallets_before);
        let wallets_after = snapshots(&self.wallets);
        let residual_btc = self.residual_btc();
        let before_usd = total_usd(&wallets_before);
        let after_usd = total_usd(&wallets_after);
        let before_btc = total_btc(&wallets_before);
        let after_btc = total_btc(&wallets_after);
        let pnl_usd = after_usd - before_usd;
        let ledger_pnl_usd = self
            .ledger
            .iter()
            .flat_map(|entry| entry.deltas.iter())
            .map(|delta| delta.usd)
            .sum();
        let replayed = replay_ledger(&self.wallets_before, &self.ledger)?;
        let fills_unique = self
            .fills
            .iter()
            .map(|fill| fill.fill_id.as_str())
            .collect::<BTreeSet<_>>()
            .len()
            == self.fills.len();
        let fill_sequences_monotonic = self
            .fills
            .iter()
            .enumerate()
            .all(|(index, fill)| fill.exchange_sequence == index as u64 + 1);
        let terminal_reconciled = state == ExecutionState::Reconciled;
        let residual_within_tolerance = residual_btc.abs() <= RESIDUAL_TOLERANCE_BTC;
        let btc_conserved = before_btc == after_btc;
        let wallet_equation =
            before_usd + pnl_usd == after_usd && before_btc + residual_btc == after_btc;
        let ledger_reconciled = replayed == self.wallets;
        let pnl_matches_ledger = pnl_usd == ledger_pnl_usd;
        let reservations_released_once = self.reservation_release_count == 1
            && wallets_after.iter().all(|wallet| {
                wallet.reserved_usd == Decimal::ZERO && wallet.reserved_btc == Decimal::ZERO
            });
        let no_negative_balances = wallets_after.iter().all(|wallet| {
            wallet.usd >= Decimal::ZERO
                && wallet.btc >= Decimal::ZERO
                && wallet.reserved_usd >= Decimal::ZERO
                && wallet.reserved_btc >= Decimal::ZERO
                && wallet.reserved_usd <= wallet.usd
                && wallet.reserved_btc <= wallet.btc
        });
        let all_passed = terminal_reconciled
            && residual_within_tolerance
            && btc_conserved
            && wallet_equation
            && ledger_reconciled
            && pnl_matches_ledger
            && fills_unique
            && fill_sequences_monotonic
            && reservations_released_once
            && no_negative_balances;
        Ok(ExecutionReport {
            execution_id: self.request.execution_id,
            scenario: self.request.scenario,
            state,
            wallets_before,
            wallets_after,
            fills: self.fills,
            ledger: self.ledger,
            transitions: self.transitions,
            pnl_usd,
            ledger_pnl_usd,
            residual_before_recovery_btc: self.residual_before_recovery_btc,
            residual_btc,
            selected_recovery: self.selected_recovery,
            invariants: ExecutionInvariants {
                terminal_reconciled,
                residual_within_tolerance,
                btc_conserved,
                wallet_equation,
                ledger_reconciled,
                pnl_matches_ledger,
                fills_unique,
                fill_sequences_monotonic,
                reservations_released_once,
                no_negative_balances,
                all_passed,
            },
            duplicates_ignored: self.duplicates_ignored,
            out_of_order_ignored: self.out_of_order_ignored,
            reservation_release_count: self.reservation_release_count,
            reservation_retries_ignored: self.reservation_retries_ignored,
            restart_performed: self.restart_performed,
            retry_performed: self.retry_performed,
            state_rehydrated: self.state_rehydrated,
        })
    }

    fn append_ledger(
        &mut self,
        kind: LedgerEntryKind,
        reference: String,
        deltas: Vec<WalletDelta>,
        detail: impl Into<String>,
    ) {
        self.ledger.push(WalletLedgerEntry {
            sequence: self.ledger.len() as u64 + 1,
            kind,
            reference,
            deltas,
            detail: detail.into(),
        });
    }

    fn wallet(&self, venue: &str) -> Result<&WalletSnapshot, ExecutionError> {
        self.wallets
            .get(venue)
            .ok_or_else(|| ExecutionError::Internal {
                message: format!("wallet inexistente para {venue}"),
            })
    }

    fn wallet_mut(&mut self, venue: &str) -> Result<&mut WalletSnapshot, ExecutionError> {
        self.wallets
            .get_mut(venue)
            .ok_or_else(|| ExecutionError::Internal {
                message: format!("wallet inexistente para {venue}"),
            })
    }

    fn available_usd(&self, venue: &str) -> Result<Decimal, ExecutionError> {
        let wallet = self.wallet(venue)?;
        Ok(wallet.usd - wallet.reserved_usd)
    }

    fn available_btc(&self, venue: &str) -> Result<Decimal, ExecutionError> {
        let wallet = self.wallet(venue)?;
        Ok(wallet.btc - wallet.reserved_btc)
    }
}

fn allowed_transition(from: Option<ExecutionState>, to: ExecutionState) -> bool {
    matches!(
        (from, to),
        (None, ExecutionState::Detected)
            | (Some(ExecutionState::Detected), ExecutionState::Reserved)
            | (
                Some(ExecutionState::Reserved),
                ExecutionState::Leg1Submitted
            )
            | (
                Some(ExecutionState::Leg1Submitted),
                ExecutionState::Leg1Partial | ExecutionState::Leg1Filled
            )
            | (
                Some(ExecutionState::Leg1Partial | ExecutionState::Leg1Filled),
                ExecutionState::Leg2Submitted | ExecutionState::RecoverySelected
            )
            | (
                Some(ExecutionState::Leg2Submitted),
                ExecutionState::Leg2Partial
                    | ExecutionState::Leg2Filled
                    | ExecutionState::Leg2Rejected
                    | ExecutionState::Leg2TimedOut
            )
            | (
                Some(
                    ExecutionState::Leg2Partial
                        | ExecutionState::Leg2Filled
                        | ExecutionState::Leg2Rejected
                        | ExecutionState::Leg2TimedOut
                ),
                ExecutionState::RecoverySelected | ExecutionState::Reconciled
            )
            | (
                Some(ExecutionState::RecoverySelected),
                ExecutionState::Reconciled
            )
    )
}

fn prorated_fee(total: Decimal, filled: Decimal, requested: Decimal) -> Decimal {
    if requested <= Decimal::ZERO {
        Decimal::ZERO
    } else {
        total * filled / requested
    }
}

fn wallet_delta(
    venue: &str,
    usd: Decimal,
    btc: Decimal,
    reserved_usd: Decimal,
    reserved_btc: Decimal,
) -> WalletDelta {
    WalletDelta {
        venue: venue.to_string(),
        usd,
        btc,
        reserved_usd,
        reserved_btc,
    }
}

fn snapshots(wallets: &BTreeMap<String, WalletSnapshot>) -> Vec<WalletSnapshot> {
    wallets.values().cloned().collect()
}

fn total_usd(wallets: &[WalletSnapshot]) -> Decimal {
    wallets.iter().map(|wallet| wallet.usd).sum()
}

fn total_btc(wallets: &[WalletSnapshot]) -> Decimal {
    wallets.iter().map(|wallet| wallet.btc).sum()
}

fn replay_ledger(
    initial: &BTreeMap<String, WalletSnapshot>,
    ledger: &[WalletLedgerEntry],
) -> Result<BTreeMap<String, WalletSnapshot>, ExecutionError> {
    let mut replayed = initial.clone();
    for entry in ledger {
        for delta in &entry.deltas {
            let wallet =
                replayed
                    .get_mut(&delta.venue)
                    .ok_or_else(|| ExecutionError::Internal {
                        message: format!("delta para wallet inexistente: {}", delta.venue),
                    })?;
            wallet.usd += delta.usd;
            wallet.btc += delta.btc;
            wallet.reserved_usd += delta.reserved_usd;
            wallet.reserved_btc += delta.reserved_btc;
        }
    }
    Ok(replayed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;

    fn request(scenario: ExecutionScenario) -> ExecutionRequest {
        standard_request(scenario)
    }

    #[test]
    fn matriz_de_doce_escenarios_concilia_todas_las_invariantes() {
        let reports = standard_matrix().expect("la matriz completa debe reconciliarse");
        assert_eq!(reports.len(), ExecutionScenario::ALL.len());

        for report in reports {
            let scenario = report.scenario;
            assert_eq!(report.state, ExecutionState::Reconciled, "{scenario:?}");
            assert_eq!(report.residual_btc, Decimal::ZERO, "{scenario:?}");
            assert_eq!(report.reservation_release_count, 1, "{scenario:?}");
            assert_eq!(report.pnl_usd, report.ledger_pnl_usd, "{scenario:?}");
            assert!(report.invariants.all_passed, "{scenario:?}: {report:#?}");
        }
    }

    #[test]
    fn respuestas_duplicadas_y_fuera_de_orden_no_se_aplican() {
        let duplicate = simulate(request(ExecutionScenario::DuplicateResponse))
            .expect("duplicado debe ignorarse");
        assert_eq!(duplicate.duplicates_ignored, 1);
        assert_eq!(duplicate.fills.len(), 2);

        let out_of_order = simulate(request(ExecutionScenario::OutOfOrderEvent))
            .expect("evento fuera de orden debe ignorarse");
        assert_eq!(out_of_order.out_of_order_ignored, 1);
        assert_eq!(out_of_order.fills.len(), 2);
    }

    #[test]
    fn reinicio_y_reintento_preservan_idempotencia_y_reservas() {
        let restart = simulate(request(ExecutionScenario::RestartDuringExecution))
            .expect("debe rehidratar ejecución");
        assert!(restart.restart_performed);
        assert!(restart.state_rehydrated);
        assert_eq!(restart.duplicates_ignored, 0);

        let retry = simulate(request(ExecutionScenario::IdempotentRetryAfterRestart))
            .expect("reintento debe ser idempotente");
        assert!(retry.restart_performed);
        assert!(retry.retry_performed);
        assert!(retry.state_rehydrated);
        assert_eq!(retry.duplicates_ignored, 1);
        assert_eq!(retry.reservation_retries_ignored, 1);
        assert_eq!(retry.reservation_release_count, 1);
        assert!(retry.invariants.all_passed);
    }

    #[test]
    fn selector_elige_rehedge_y_unwind_por_costo() {
        let mut rehedge = request(ExecutionScenario::RehedgeCheaper);
        rehedge.prices.rehedge_price_usd = Decimal::new(50_050, 0);
        rehedge.prices.unwind_price_usd = Decimal::new(49_950, 0);
        rehedge.costs.rehedge_cost_usd = Decimal::ONE;
        rehedge.costs.unwind_cost_usd = Decimal::TEN;
        let rehedge_report = simulate(rehedge).expect("rehedge debe ser factible");
        assert_eq!(
            rehedge_report.selected_recovery,
            Some(RecoveryAction::Rehedge)
        );

        let mut unwind = request(ExecutionScenario::UnwindCheaper);
        unwind.prices.rehedge_price_usd = Decimal::new(49_950, 0);
        unwind.prices.unwind_price_usd = Decimal::new(50_050, 0);
        unwind.costs.rehedge_cost_usd = Decimal::TEN;
        unwind.costs.unwind_cost_usd = Decimal::ONE;
        let unwind_report = simulate(unwind).expect("unwind debe ser factible");
        assert_eq!(
            unwind_report.selected_recovery,
            Some(RecoveryAction::Unwind)
        );
    }

    #[test]
    fn solicitud_invalida_falla_sin_mutar_estado_externo() {
        let mut invalid = request(ExecutionScenario::BothLegsFilled);
        invalid.quantity_btc = Decimal::ZERO;
        assert!(matches!(
            simulate(invalid),
            Err(ExecutionError::InvalidRequest { .. })
        ));
    }
}
