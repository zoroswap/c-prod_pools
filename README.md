# Miden constant-product (XYK) pools

**XYK** (“**x** times **y** equals **k**”) pools hold two token reserves so that (x * y = k) swaps move reserves along that curve and liquidity providers supply both sides of the pair.

They matter in DeFi because they offer **permissionless liquidity**: anyone can pool assets and anyone can trade against a formula, without order books or centralized market makers. That makes markets for long-tail tokens, anchors **price discovery** from supply and demand, and plugs cleanly into the rest of the stack-aggregators, lending protocols, yield strategies, and treasuries can treat a pool as a composable “swap and quote” primitive.

## How to run

Set a `.env` in the project root, for example:

```env
MIDEN_NODE_ENDPOINT="localhost"
```

(`testnet` and `devnet` are also supported; anything else falls back to localhost.)

List tests with

```bash
cargo test --release -- --list
```

Run a specific a test with

```bash
cargo test --release $TEST_NAME -- --exact --no-capture
```

If you want to run tests, use only one thread as they share the same cached users / faucets which makes tests fail when running in parallel.

```bash
cargo test --release -- --test-threads 1
```

