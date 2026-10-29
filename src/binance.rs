//! Binance: where we borrow our price from.
//!
//! Binance's SOLUSDT perpetual is far busier than any one DEX, so its price
//! moves first. We listen to its `bookTicker` stream, which pushes the best
//! bid and ask every time either changes, and forward each one to the bot.

use std::str::FromStr;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use rust_decimal::Decimal;
use serde::Deserialize;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

use crate::events::{Bus, Event};

/// The JSON Binance sends. Prices arrive as strings so no precision is lost.
#[derive(Deserialize)]
struct BookTicker {
    #[serde(rename = "b")]
    bid: String,
    #[serde(rename = "a")]
    ask: String,
}

/// Start sending `symbol`'s (e.g. "SOLUSDT") best bid/ask from Binance USD-M
/// futures to `bus`. Runs forever in the background, reconnecting if the
/// connection drops.
pub fn connect(symbol: &str, bus: Bus) {
    let url = format!(
        "wss://fstream.binance.com/ws/{}@bookTicker",
        symbol.to_lowercase()
    );
    tokio::spawn(run(url, bus));
}

async fn run(url: String, bus: Bus) {
    let mut backoff = Duration::from_secs(1);
    loop {
        match connect_async(&url).await {
            Ok((mut ws, _)) => {
                tracing::info!("binance: connected");
                backoff = Duration::from_secs(1);
                while let Some(msg) = ws.next().await {
                    match msg {
                        Ok(Message::Text(text)) => match parse(&text) {
                            Ok(event) => {
                                if bus.send(event).is_err() {
                                    return; // the bot has shut down
                                }
                            }
                            Err(e) => tracing::warn!("binance: bad message: {e}"),
                        },
                        Ok(Message::Ping(payload)) => {
                            if ws.send(Message::Pong(payload)).await.is_err() {
                                break;
                            }
                        }
                        Ok(Message::Close(_)) | Err(_) => break,
                        Ok(_) => {}
                    }
                }
                tracing::warn!("binance: disconnected");
            }
            Err(e) => tracing::warn!("binance: connect failed: {e}"),
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(30));
    }
}

fn parse(text: &str) -> anyhow::Result<Event> {
    let ticker: BookTicker = serde_json::from_str(text)?;
    Ok(Event::Reference {
        bid: Decimal::from_str(&ticker.bid)?,
        ask: Decimal::from_str(&ticker.ask)?,
        received_at: Instant::now(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_real_book_ticker_message() {
        let msg = r#"{"e":"bookTicker","u":1,"s":"SOLUSDT","b":"150.10","B":"20","a":"150.12","A":"7","T":1,"E":1}"#;
        let Event::Reference { bid, ask, .. } = parse(msg).unwrap() else {
            panic!()
        };
        assert_eq!((bid, ask), (Decimal::new(15010, 2), Decimal::new(15012, 2)));
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse("not json").is_err());
        assert!(parse(r#"{"b":"oops","a":"1"}"#).is_err());
    }
}
