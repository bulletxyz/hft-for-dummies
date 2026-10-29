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

use crate::events::{Bus, Event, Stamp};

/// The JSON Binance sends. Prices arrive as strings so no precision is lost.
#[derive(Deserialize)]
struct BookTicker {
    #[serde(rename = "b")]
    bid: String,
    #[serde(rename = "a")]
    ask: String,
    /// When Binance sent this message (ms since 1970).
    #[serde(rename = "E")]
    event_ms: Option<u64>,
    /// When the book change behind it happened (ms since 1970).
    #[serde(rename = "T")]
    match_ms: Option<u64>,
}

/// Start sending `symbol`'s (e.g. "SOLUSDT") best bid/ask from Binance USD-M
/// futures to `bus`. Runs forever, reconnecting if the connection drops.
///
/// The feed gets its own OS thread with its own small async runtime, so
/// reading the price never waits behind anything else the bot is doing (like
/// an order request on the main runtime).
pub fn connect(symbol: &str, bus: Bus) {
    let url = format!(
        "wss://fstream.binance.com/ws/{}@bookTicker",
        symbol.to_lowercase()
    );
    std::thread::Builder::new()
        .name("binance-feed".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("building the feed's runtime");
            runtime.block_on(run(url, bus));
        })
        .expect("starting the feed thread");
}

/// We ping Binance every second. A healthy connection answers within
/// milliseconds; hearing nothing at all (no prices, no pong) for this long
/// means it's dead, even if the operating system hasn't noticed: a wifi blip
/// or a dropped route can leave a socket half-open for minutes.
const DEAD_AFTER: Duration = Duration::from_secs(3);

async fn run(url: String, bus: Bus) {
    let mut backoff = Duration::from_secs(1);
    loop {
        match tokio::time::timeout(DEAD_AFTER, connect_async(&url)).await {
            Ok(Ok((mut ws, _))) => {
                tracing::info!("binance: connected");
                backoff = Duration::from_secs(1);
                let mut ping = tokio::time::interval(Duration::from_secs(1));
                let mut last_heard = Instant::now();
                loop {
                    let msg = tokio::select! {
                        msg = ws.next() => match msg {
                            Some(msg) => msg,
                            None => break,
                        },
                        _ = ping.tick() => {
                            if last_heard.elapsed() > DEAD_AFTER {
                                tracing::warn!("binance: nothing heard for {}s", DEAD_AFTER.as_secs());
                                break;
                            }
                            if ws.send(Message::Ping(Vec::new().into())).await.is_err() {
                                break;
                            }
                            continue;
                        }
                    };
                    last_heard = Instant::now();
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
                        Ok(_) => {} // pongs: `last_heard` already updated
                    }
                }
                tracing::warn!("binance: disconnected, reconnecting");
            }
            Ok(Err(e)) => tracing::warn!("binance: connect failed: {e}"),
            Err(_) => tracing::warn!("binance: connect timed out"),
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
        stamp: Stamp::now(
            ticker.event_ms.map(|ms| ms * 1000),
            ticker.match_ms.map(|ms| ms * 1000),
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_real_book_ticker_message() {
        let msg = r#"{"e":"bookTicker","u":1,"s":"SOLUSDT","b":"150.10","B":"20","a":"150.12","A":"7","T":1,"E":1}"#;
        let Event::Reference { bid, ask, stamp } = parse(msg).unwrap() else {
            panic!()
        };
        assert_eq!((bid, ask), (Decimal::new(15010, 2), Decimal::new(15012, 2)));
        // Binance times are ms; we keep µs.
        assert_eq!(
            (stamp.exchange_unix_us, stamp.exchange_match_unix_us),
            (Some(1000), Some(1000))
        );
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse("not json").is_err());
        assert!(parse(r#"{"b":"oops","a":"1"}"#).is_err());
    }
}
