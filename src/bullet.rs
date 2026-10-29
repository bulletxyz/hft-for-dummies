//! Bullet: where we trade.
//!
//! One websocket brings us the order book and (when trading for real) our own
//! order updates and fills. Orders go out as signed transactions through the
//! Bullet SDK.
//!
//! ## Keys and delegation
//!
//! The bot never holds your wallet's own key. In the Bullet app you add a
//! *delegate* key to your account (or let `cargo run -- keygen` make one).
//! A delegate can place and cancel orders for the account, but can't
//! withdraw. If it ever leaks, your funds stay put.

use std::str::FromStr;

use anyhow::{Context, Result, anyhow, bail};
use bullet_rust_sdk::codegen::types::Filter;
use bullet_rust_sdk::types::{OrderUpdateData, OrderUpdateMessage};
use bullet_rust_sdk::{
    CancelOrderArgs, Client, ClientOrderId, Keypair, MarketId, Network, NewOrderArgs, OrderType,
    OrderbookDepth, PositiveDecimal, ServerMessage, Side as BulletSide, Topic,
    UserActionDiscriminants, WsEvent,
};
use rust_decimal::Decimal;
use serde::Deserialize;

use crate::book::{BookEvent, BookKeeper};
use crate::config::Config;
use crate::events::{Bus, Event, Fill, Order, Side};

pub struct Bullet {
    client: Client,
    market_id: MarketId,
    symbol: String,
    /// The account we trade (`None` in dry run).
    account: Option<String>,
    /// Prices must be a multiple of this.
    pub tick: Decimal,
}

/// Connect, and start sending the book (and, live, our order updates) to `bus`.
pub async fn connect(config: &Config, bus: Bus) -> Result<Bullet> {
    // Dry run needs no key: the order book is public.
    let keypair = if config.dry_run {
        None
    } else {
        Some(crate::key::load(&config.key_file)?)
    };
    let signer = keypair.as_ref().map(Keypair::address);
    let client = Client::builder()
        .network(Network::from(config.network.as_str()))
        .maybe_keypair(keypair)
        .user_actions(vec![
            UserActionDiscriminants::PlaceOrders,
            UserActionDiscriminants::CancelOrders,
            UserActionDiscriminants::CancelMarketOrders,
        ])
        .build()
        .await
        .context("connecting to Bullet")?;

    let account = match signer {
        Some(signer) => Some(trading_account(client.url(), &config.network, &signer).await?),
        None => None,
    };
    let market_id = client
        .market_id(&config.symbol)
        .ok_or_else(|| anyhow!("Bullet has no market called {:?}", config.symbol))?;
    let (tick, step, min_size) = market_rules(&client, &config.symbol).await?;
    if config.order_size < min_size {
        bail!(
            "order_size {} is below {}'s minimum of {min_size}",
            config.order_size,
            config.symbol
        );
    }
    if !step.is_zero() && !(config.order_size % step).is_zero() {
        bail!(
            "order_size {} must be a multiple of {}'s size step {step}",
            config.order_size,
            config.symbol
        );
    }
    tracing::info!(symbol = %config.symbol, %tick, %step, %min_size, account = ?account, "bullet: connected");

    let mut topics = vec![Topic::depth(config.symbol.as_str(), OrderbookDepth::D20)];
    if let Some(account) = &account {
        topics.push(Topic::user_orders(account.clone()));
    }
    let mut ws = client
        .connect_ws_managed()
        .call()
        .await
        .context("opening Bullet websocket")?;
    ws.subscribe(topics, None).context("subscribing")?;

    let symbol = config.symbol.clone();
    tokio::spawn(async move {
        let mut book = BookKeeper::default();
        loop {
            let event = match ws.recv().await {
                Some(WsEvent::Message(msg)) => match *msg {
                    ServerMessage::DepthUpdate(depth) if depth.symbol == symbol => {
                        match book.on_message(&depth) {
                            BookEvent::Updated(book) => Some(Event::Book(book)),
                            BookEvent::Lost => Some(Event::BookLost),
                            BookEvent::None => None,
                        }
                    }
                    ServerMessage::OrderUpdate(update) => {
                        tracing::debug!(?update, "bullet: order update");
                        order_event(&update, &symbol)
                    }
                    _ => None,
                },
                Some(WsEvent::Reconnecting) => {
                    book.reset();
                    Some(Event::Reconnecting)
                }
                Some(WsEvent::Disconnected(why)) => Some(Event::Disconnected(why)),
                None => Some(Event::Disconnected("websocket closed".into())),
            };
            let done = matches!(event, Some(Event::Disconnected(_)));
            if let Some(event) = event
                && bus.send(event).is_err()
            {
                return;
            }
            if done {
                return;
            }
        }
    });

    Ok(Bullet {
        client,
        market_id,
        symbol: config.symbol.clone(),
        account,
        tick,
    })
}

impl Bullet {
    /// Place post-only orders. Post-only means "rest on the book or be
    /// rejected", so we only ever earn the spread, never pay to cross it.
    pub async fn place(&self, orders: &[Order]) -> Result<()> {
        let orders = orders
            .iter()
            .map(|o| {
                Ok(NewOrderArgs {
                    price: PositiveDecimal::try_from(o.price)
                        .map_err(|e| anyhow!("price {}: {e}", o.price))?,
                    size: PositiveDecimal::try_from(o.size)
                        .map_err(|e| anyhow!("size {}: {e}", o.size))?,
                    side: match o.side {
                        Side::Buy => BulletSide::Bid,
                        Side::Sell => BulletSide::Ask,
                    },
                    order_type: OrderType::PostOnly,
                    reduce_only: false,
                    client_order_id: Some(ClientOrderId(o.client_id)),
                    pending_tpsl_pair: None,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        self.client
            .place_orders(self.market_id, orders, false, None)
            .await?;
        Ok(())
    }

    pub async fn cancel(&self, client_ids: &[u64]) -> Result<()> {
        let cancels = client_ids
            .iter()
            .map(|&id| CancelOrderArgs {
                order_id: None,
                client_order_id: Some(ClientOrderId(id)),
            })
            .collect();
        self.client
            .cancel_orders(self.market_id, cancels, None)
            .await?;
        Ok(())
    }

    /// Cancel every order we have on this market, tracked or not.
    pub async fn cancel_all(&self) -> Result<()> {
        self.client
            .cancel_market_orders(self.market_id, None)
            .await?;
        Ok(())
    }

    /// Our fills on this market since `since_unix_us`, from the exchange's
    /// own record (the most recent 1000).
    pub async fn fills_since(&self, since_unix_us: u64) -> Result<Vec<Fill>> {
        let account = self
            .account
            .as_deref()
            .ok_or_else(|| anyhow!("no account in dry run"))?;
        let body = reqwest::Client::new()
            .get(format!("{}/fapi/v1/userTrades", self.client.url()))
            .query(&[("address", account), ("symbol", &self.symbol)])
            .query(&[("startTime", since_unix_us), ("limit", 1000)])
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        parse_user_trades(&body)
    }

    /// Our position according to the exchange (positive = long).
    pub async fn position(&self) -> Result<Decimal> {
        let account = self
            .account
            .as_deref()
            .ok_or_else(|| anyhow!("no account in dry run"))?;
        let info = self
            .client
            .account_info(account)
            .await
            .context("reading the account (has it been funded?)")?
            .into_inner();
        Ok(info
            .positions
            .iter()
            .find(|p| p.symbol == self.symbol)
            .map_or(Decimal::ZERO, |p| p.position_amt))
    }
}

/// Tick size, size step and minimum size for `symbol`.
async fn market_rules(client: &Client, symbol: &str) -> Result<(Decimal, Decimal, Decimal)> {
    let info = client
        .exchange_info()
        .await
        .context("reading market rules")?
        .into_inner();
    let market = info
        .symbols
        .iter()
        .find(|s| s.symbol == symbol)
        .ok_or_else(|| anyhow!("{symbol} is not listed"))?;
    let (mut tick, mut step, mut min_size) = (None, None, Decimal::ZERO);
    for filter in &market.filters {
        match filter {
            Filter::PriceFilter { tick_size, .. } => tick = Some(*tick_size),
            Filter::LotSize {
                step_size, min_qty, ..
            } => {
                step = Some(*step_size);
                min_size = *min_qty;
            }
            _ => {}
        }
    }
    Ok((
        tick.ok_or_else(|| anyhow!("{symbol} has no tick size"))?,
        step.ok_or_else(|| anyhow!("{symbol} has no size step"))?,
        min_size,
    ))
}

/// `GET /fapi/v1/userTrades` rows as [`Fill`]s. A row doesn't say whether the
/// order is now finished, so these never clear a resting order; if one did
/// finish, the next cancel for it fails and the bot pulls and requotes.
fn parse_user_trades(body: &str) -> Result<Vec<Fill>> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Row {
        id: u64,
        client_order_id: Option<u64>,
        side: String,
        price: Decimal,
        qty: Decimal,
        commission: Decimal,
    }
    #[derive(Deserialize)]
    struct Page {
        data: Vec<Row>,
    }
    let page: Page = serde_json::from_str(body).context("reading userTrades")?;
    page.data
        .into_iter()
        .map(|row| {
            let side = match row.side.as_str() {
                "BUY" => Side::Buy,
                "SELL" => Side::Sell,
                other => bail!("userTrades: unknown side {other:?}"),
            };
            Ok(Fill {
                trade_id: row.id,
                client_id: row.client_order_id,
                side,
                price: row.price,
                size: row.qty,
                fee: row.commission,
                order_finished: false,
            })
        })
        .collect()
}

/// Turn one of our order updates into an [`Event`].
fn order_event(update: &OrderUpdateMessage, symbol: &str) -> Option<Event> {
    match &update.order {
        OrderUpdateData::TradeFill(fill) if fill.common.symbol == symbol => {
            let side = match fill.side.as_str() {
                "BUY" => Side::Buy,
                "SELL" => Side::Sell,
                other => {
                    tracing::error!("bullet: fill with unknown side {other:?}, ignoring it");
                    return None;
                }
            };
            let (Ok(price), Ok(size)) = (
                Decimal::from_str(&fill.last_filled_price),
                Decimal::from_str(&fill.last_filled_qty),
            ) else {
                tracing::error!("bullet: unreadable fill {fill:?}, position may now be wrong");
                return None;
            };
            Some(Event::Fill(Fill {
                trade_id: fill.trade_id,
                client_id: fill.common.client_order_id,
                side,
                price,
                size,
                fee: Decimal::from_str(&fill.commission).unwrap_or_default(),
                order_finished: fill.common.status == "FILLED",
            }))
        }
        OrderUpdateData::Cancel(cancel) if cancel.common.symbol == symbol => cancel
            .common
            .client_order_id
            .map(|client_id| Event::OrderGone { client_id }),
        _ => None,
    }
}

/// The account a delegated key trades for. Refuses a key that isn't a delegate:
/// the bot never runs on a wallet's own key.
async fn trading_account(rest_url: &str, network: &str, signer: &str) -> Result<String> {
    let http = reqwest::Client::new();
    let resp = http
        .get(format!("{rest_url}/api/v1/delegateOf"))
        .query(&[("address", signer)])
        .send()
        .await
        .context("checking the key's delegation")?;
    let status = resp.status().as_u16();
    let body = resp.text().await?;
    let account = match status {
        200 => serde_json::from_str::<DelegateOf>(&body)?.parent,
        // Not a delegate: the API says 404, live servers say 400 "... is not a delegate".
        _ if status == 404 || (status == 400 && body.contains("is not a delegate")) => bail!(
            "the key in your key file has address {signer}, and that address isn't a delegate.\n  \
             - Pasted from the Bullet app? Check you copied the PRIVATE key, not the address,\n    \
               and that network = \"{network}\" matches where you created it.\n  \
             - Made it with keygen? Add it as a delegate here:\n    {}",
            delegate_link(network, signer)
        ),
        _ => bail!("checking the delegation failed: HTTP {status}: {body}"),
    };

    tracing::info!(signer, account, "bullet: key is delegated");
    Ok(account)
}

/// A link that opens the Bullet app's delegate page with this key filled in.
pub fn delegate_link(network: &str, signer: &str) -> String {
    let host = if network == "testnet" {
        "https://app.testnet.bullet.xyz"
    } else {
        "https://app.bullet.xyz"
    };
    format!("{host}/delegate-accounts?delegate={signer}&name=hft-for-dummies")
}

#[derive(Deserialize)]
struct DelegateOf {
    parent: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_fills_from_the_exchange_record() {
        // A real row from mainnet (the fill our websocket missed on 2026-10-29).
        let body = r#"{"data":[{"symbol":"SOL-USD","id":23168826,"orderId":5599961285,"clientOrderId":1793276669648035,"side":"SELL","price":"100.52","qty":"0.05","quoteQty":"5.026","realizedPnl":"0","commission":"0","commissionAsset":"USDC","time":1793277010300319,"positionSide":"BOTH","buyer":false,"maker":true,"isDegen":false}]}"#;
        let fills = parse_user_trades(body).unwrap();
        assert_eq!(fills.len(), 1);
        let f = &fills[0];
        assert_eq!(
            (f.trade_id, f.client_id, f.side),
            (23_168_826, Some(1_793_276_669_648_035), Side::Sell)
        );
        assert_eq!(
            (f.price, f.size),
            (Decimal::new(10052, 2), Decimal::new(5, 2))
        );
        assert!(!f.order_finished);
    }

    #[test]
    fn an_empty_page_is_no_fills() {
        assert!(parse_user_trades(r#"{"data":[]}"#).unwrap().is_empty());
    }
}
