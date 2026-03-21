# Miden constant-product (XYK) pools

**XYK** (“**x** times **y** equals **k**”) pools hold two token reserves so that \(x \cdot y = k\); swaps move reserves along that curve and liquidity providers supply both sides of the pair.

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

Run specific a test with

```bash
cargo test --release $TEST_NAME -- --exact --no-capture
```

If you want to run tests, use only one thread as they share the same cached users / faucets which makes tests fail when running in parallel.

```bash
cargo test --release -- --test-threads 1
```

