# HFT for dummies

The code that goes with the blog series *HFT for dummies*. It starts as the smallest
useful market maker and grows a piece at a time as the series does.

## Part 1: a reference-price market maker

- `src/quoter.rs`: the whole strategy. Quote a bid and an ask around a reference price,
  lean against inventory, stop quoting a side at the position limit.
- `src/sim.rs`: a simulated market, so there are no keys, no network and no money at risk.
- `src/main.rs`: runs it and prints position and PnL.

```sh
cargo run --release
cargo test
```

The simulation hands the quoter the true fair value as its reference price. A real
market maker has to estimate that, which is where the series goes next.

## Where it's going

Real trading, starting on testnet. Each part of the series changes this crate in small steps.

## License

Apache-2.0.
