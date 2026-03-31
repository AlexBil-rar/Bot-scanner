use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum OrderSide { Buy, Sell }

#[derive(Debug, Clone, PartialEq)]
pub enum OrderStatus {
    Pending,
    Filled { fill_price: f64, fill_qty: f64 },
    PartialFill { fill_price: f64, fill_qty: f64 },
    Rejected { reason: String },
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct Order {
    pub exchange: String,
    pub symbol: String,    // "BTC", "ETH", etc
    pub side: OrderSide,
    pub quantity: f64,
    pub price: f64,        // limit price (для market — 0.0)
    pub is_market: bool,
    pub client_order_id: String,
    pub status: OrderStatus,
    pub created_at_ms: u64,
    pub filled_at_ms: Option<u64>,
}

impl Order {
    pub fn market_buy(exchange: &str, symbol: &str, quantity: f64) -> Self {
        Self {
            exchange: exchange.to_string(),
            symbol: symbol.to_string(),
            side: OrderSide::Buy,
            quantity,
            price: 0.0,
            is_market: true,
            client_order_id: generate_id(),
            status: OrderStatus::Pending,
            created_at_ms: now_ms(),
            filled_at_ms: None,
        }
    }

    pub fn market_sell(exchange: &str, symbol: &str, quantity: f64) -> Self {
        Self {
            exchange: exchange.to_string(),
            symbol: symbol.to_string(),
            side: OrderSide::Sell,
            quantity,
            price: 0.0,
            is_market: true,
            client_order_id: generate_id(),
            status: OrderStatus::Pending,
            created_at_ms: now_ms(),
            filled_at_ms: None,
        }
    }

    pub fn is_filled(&self) -> bool {
        matches!(self.status, OrderStatus::Filled { .. })
    }
}

impl fmt::Display for Order {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {:?} {} {} @ {}",
            self.exchange, self.side, self.quantity, self.symbol,
            if self.is_market { "MARKET".to_string() } else { format!("{:.4}", self.price) }
        )
    }
}

/// Результат исполнения арбитражной сделки (две ноги)
#[derive(Debug)]
pub struct ArbitrageExecution {
    pub buy_order: Order,
    pub sell_order: Order,
    pub signal_profit_pct: f64,
    pub actual_profit_usd: f64,
    pub latency_ms: u64,  // время от сигнала до исполнения
    pub success: bool,
}

impl fmt::Display for ArbitrageExecution {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f,
            "ARB {} | buy={} sell={} | expected={:.4}% actual=${:.4} | latency={}ms | {}",
            self.buy_order.symbol,
            self.buy_order.exchange,
            self.sell_order.exchange,
            self.signal_profit_pct,
            self.actual_profit_usd,
            self.latency_ms,
            if self.success { "✅ SUCCESS" } else { "❌ FAILED" }
        )
    }
}

fn generate_id() -> String {
    format!("arb-{}", now_ms())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}
