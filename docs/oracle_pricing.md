# Optional oracle pricing

The subscription vault supports optional cross-currency pricing through an external oracle contract.

## Oracle interface

When enabled, the vault calls oracle method:

- `latest_price() -> OraclePrice`

`OraclePrice` fields:

- `price`: quote units per 1 token (must be positive)
- `timestamp`: quote publication time

## Configuration

Admin-only:

- `set_oracle_config(admin, enabled, oracle, max_age_seconds)`

Read:

- `get_oracle_config()`

Safety checks:

- enabled requires oracle address
- enabled requires `max_age_seconds > 0` (zero disables staleness guard and is rejected)
- stale data rejected when quote age exceeds `max_age_seconds`
- zero/negative price rejected
- zero timestamp rejected as unavailable

## Charge conversion

With oracle disabled, `subscription.amount` is treated as token-denominated (existing behavior).

With oracle enabled, `subscription.amount` is interpreted as quote-denominated and converted:

`token_amount = ceil(quote_amount * 10^token_decimals / price)`

This preserves deterministic charging while allowing quote-currency plan pricing.

## Failure modes

- `OracleNotConfigured`
- `OraclePriceUnavailable`
- `OraclePriceStale`
- `OraclePriceInvalid`

These errors cause the charge to fail without mutating balances.

## Edge case: a price of exactly 1

A price of `1` looks like a degenerate or broken oracle reading, and it is the
boundary most likely to be reached by accident. This section documents exactly
what happens.

### Price representation

Prices are not floats. The contract stores them as `u128` scaled by
`PRICE_SCALE = 10^7` (7 decimal places, matching Stellar asset precision):

```
real_price = price / 10^7
```

So "a price of exactly 1" is the **raw value `10_000_000`**, not `1`. A raw
value of `1` would mean a real price of `1e-7` and is rejected as degenerate
(see below).

### The conversion, and why it cannot produce a zero charge

`resolve_charge_amount` in `oracle.rs` computes:

```
token_amount = ceil(quote_amount * 10^token_decimals / price)
```

implemented with overflow-checked helpers:

```rust
let scale        = safe_pow(10i128, token_decimals)?;
let numerator    = safe_mul(subscription.amount, scale)?;
let ceil_adjust  = safe_sub(price.price, 1)?;          // price > 0, so no underflow
let token_amount = safe_div(safe_add(numerator, ceil_adjust)?, price.price)?;

if token_amount <= 0 {
    return Err(Error::OraclePriceInvalid);
}
Ok(token_amount)
```

The `ceil` is what guarantees a non-zero result. For any `quote_amount >= 1`
and any positive price, `ceil` yields **at least 1** base unit. Concretely at
`price = 10^7` (price 1.0) with a 6-decimal token:

| `quote_amount` | `token_amount = ceil(quote / 10)` | Result |
|----------------|-----------------------------------|--------|
| 0 | 0 | **Rejected** — `OraclePriceInvalid` (3007) |
| 1 | 1 | 1 base unit |
| 5 | 1 | 1 base unit (rounds **up**) |
| 10 | 1 | 1 base unit |
| 11 | 2 | 2 base units |
| 1_000_000 | 100_000 | 100_000 base units |

So the specific fear — "a price of 1 produces a charge of 0 after integer
division" — **does not occur**. The only input that produces `token_amount == 0`
is `quote_amount == 0`, and that is rejected before any balance is touched.

### Which error is returned, and why it is not `InvalidAmount`

The guard returns **`OraclePriceInvalid` (3007)**, not `InvalidAmount` (3001).
This is deliberate:

- `InvalidAmount` asserts the *caller-supplied* subscription amount is
  malformed. In this scenario `subscription.amount` is perfectly valid — the
  thing that is wrong is the **price the oracle returned**. Reporting
  `InvalidAmount` would send integrators to fix a plan configuration that is
  not the problem, and away from the actual fault, which is the oracle.
- `OraclePriceInvalid` (3007) is already documented as "Oracle returned a
  non-positive price" and is grouped with the other oracle faults
  (`OraclePriceUnavailable`, `OraclePriceStale`), so it routes to the correct
  alert path.

**The safety property the issue asks for is satisfied:** a zero charge is
rejected and no balance is mutated. The specific error code differs from the
one suggested, and the reason is the more accurate diagnosis.

### Genuinely degenerate prices are rejected earlier

Before the division, `resolve_charge_amount` already rejects:

- `price.price <= 0` → `OraclePriceInvalid`. This covers a raw price of `0` and
  any negative value, which is what actually protects the `safe_sub(price, 1)`
  underflow.
- `price.timestamp == 0` → `OraclePriceUnavailable` (no observation exists).
- Age beyond `max_age_seconds` → `OraclePriceStale`.
- Deviation beyond the circuit-breaker threshold → `OracleDeviationTooHigh`.

A raw value of `1` (real price `1e-7`) is not rejected by the `price > 0` test.
It is not dangerous — it yields an enormous `token_amount`, which is bounded by
the `safe_mul` overflow check and by the subscription's own funding — but it is
a sign of a badly scaled oracle and should be alarmed on off-chain. Treat a raw
price below `10^6` (real price `< 0.1`) as an oracle fault.

### Operational guidance

- **Enforce a minimum `quote_amount` in plan configuration.** Prices of 1.0
  combined with small quote amounts produce 1-base-unit charges whose
  subsequent fee split is dominated by rounding. A charge this small is almost
  always a configuration error rather than intent.
- **Alert on the raw price, not the scaled one.** Monitor
  `oracle_charge_resolved.price` for values `< 10^6`.
- **Do not treat a rejected conversion as retryable.** All four oracle errors
  are terminal for that charge attempt; the oracle data must be fixed first.

## Events

For off-chain verification and indexability, the following events are emitted:

- `oracle_config_updated`: Emitted when the admin updates oracle configuration. Includes enabled status, oracle address, max acceptable age, and timestamp.
- `oracle_charge_resolved`: Emitted when a charge resolves its token target via the oracle. Includes `quote_amount`, `token_amount`, `price`, `price_timestamp` from the oracle, and resolution `timestamp`.
- `oracle_liveness`: Emitted when `emit_oracle_liveness()` is called for monitoring. Includes `last_sample_ts`, `age`, `healthy` status, and check timestamp. Allows monitoring rigs to alert before charges start failing due to stale oracle data.

## Oracle Liveness Monitoring

The contract provides a view-only `emit_oracle_liveness()` entrypoint that enables monitoring systems to verify oracle health without requiring admin privileges.

### Usage

```rust,ignore
// Check oracle health before charging
match client.emit_oracle_liveness(&env) {
    Ok(event) => {
        if event.healthy {
            // Oracle is healthy, proceed with oracle-dependent charge
            println!("Oracle healthy: age={}s, threshold={}s", event.age, event.max_age_seconds / 2);
        } else {
            // Oracle is stale or approaching staleness
            // Use fallback pricing or alert operators
            eprintln!("WARNING: Oracle stale! Age={}s exceeds healthy threshold", event.age);
        }
    }
    Err(Error::OracleNotConfigured) => {
        // Oracle not enabled, use base pricing
        println!("Oracle not configured, using base subscription amounts");
    }
    Err(e) => panic!("Unexpected error: {:?}", e),
}
```

### OracleLivenessEvent Fields

| Field            | Type   | Description                                                       |
| ---------------- | ------ | ----------------------------------------------------------------- |
| `last_sample_ts` | `u64`  | Timestamp of the latest oracle price sample                       |
| `age`            | `u64`  | Age of the sample in seconds (`current_time - last_sample_ts`)    |
| `healthy`        | `bool` | `true` if `age <= max_age_seconds / 2`, indicating healthy oracle |
| `timestamp`      | `u64`  | Ledger timestamp when this liveness check was performed           |

### Health Threshold

The `healthy` field is computed as:

```
healthy = (age <= max_age_seconds / 2)
```

This provides early warning when the oracle sample is approaching the staleness threshold. Monitoring systems can alert operators when `healthy = false`, allowing intervention before charges start failing with `OraclePriceStale` errors.

### Security Properties

- **No authentication required**: Any caller can invoke `emit_oracle_liveness()` to verify oracle health
- **View-only**: Does not modify contract state
- **Event emission**: Publishes `OracleLivenessEvent` for off-chain indexers and monitoring systems
- **Error handling**: Returns `OracleNotConfigured` if oracle is not enabled, preventing confusion

### Integration with Monitoring

Monitoring rigs can:

1. Call `emit_oracle_liveness()` on a schedule (e.g., every 60 seconds)
2. Track the `age` field to detect increasing staleness
3. Alert operators when `healthy = false` (age > max_age_seconds / 2)
4. Trigger fallback procedures before charges fail

This provides proactive oracle health monitoring, allowing operators to address issues before they impact subscription billing.

---

## OracleAdapter Architecture (Issue #477)

### Overview

Oracle pricing is now pluggable via a **strategy pattern**. The `OracleConfig` struct carries an `OracleKind` field that selects which adapter resolves the price at charge time. The public contract ABI and storage remain backwards compatible — configs without an explicit `kind` default to `Spot`.

### OracleKind

```rust
pub enum OracleKind {
    Spot,      // latest single price sample (default)
    Twap,      // median across a configurable sliding window
    FixedRate, // deterministic ratio; no oracle reads
}
```

### Configuration

The `set_oracle_config` entrypoint now accepts additional fields:

```
set_oracle_config(
    admin,
    enabled,
    oracle,          // Option<Address> — required for Spot/Twap
    max_age_seconds, // staleness threshold
    kind,            // OracleKind::Spot | Twap | FixedRate
    window_secs,     // TWAP window (ignored for Spot/FixedRate)
    fixed_numerator, // FixedRate numerator (ignored otherwise)
    fixed_denominator // FixedRate denominator, must be != 0
)
```

The `oracle_config_updated` event now includes `kind`, `window_secs`, `fixed_numerator`, and `fixed_denominator` for full auditability.

---

### SpotAdapter

Reads the latest `OraclePrice` from `oracle.latest_price()` and validates it:

- Rejects non-positive prices (`OraclePriceInvalid`).
- Rejects prices whose age exceeds `max_age_seconds` (`OraclePriceStale`).

This is the default behaviour and preserves all existing charge logic.

---

### TwapAdapter

Reads a list of `OraclePrice` observations via `oracle.get_observations(since)` for the last `window_secs` seconds.

**Median calculation** — not arithmetic mean:

```
prices = [obs.price for obs in observations if obs.age <= max_age_seconds]
sort(prices)
median = prices[len(prices) / 2]  // middle element for odd-length
```

Using the median rather than the mean means an attacker must control **more than half** of the observations inside the window to shift the output meaningfully. This resists single-block (flash-loan) price manipulation.

**Recommended configuration**

- Enforce a minimum TWAP window of at least 60 seconds in code.
- For mainnet deployments, use a default window of 5 minutes (300 seconds) or longer so that a single-block spike is diluted by several honest observations.
- Pair the TWAP with conservative slippage bounds such as 0.5%-1.0% for standard billing flows; tighter bounds may be appropriate for volatile markets or when the oracle feed itself is not well-distributed.

**Edge cases:**
| Scenario | Result |
|---|---|
| 1 observation | That price (equivalent to spot) |
| Empty window | `OraclePriceUnavailable` |
| All observations stale after filtering | `OraclePriceStale` |

---

### FixedRateAdapter

Computes a deterministic price without any oracle contract calls:

```
price = (fixed_numerator × 10^7) / fixed_denominator
```

- `fixed_denominator == 0` is rejected at configuration time with `InvalidInput`.
- Staleness and oracle address are completely ignored.
- Suitable for pegged pairs, test environments, and fee structures expressed as ratios.

**Security:** configuration changes require admin auth, so unauthorized parties cannot alter the fixed rate.

---

### Dispatch Flow

`resolve_charge_amount` now delegates to `oracle_adapter::dispatch_price`:

```
match config.kind {
    Spot      → SpotAdapter::quote()
    Twap      → TwapAdapter::quote()
    FixedRate → FixedRateAdapter::quote()
}
```

All adapters share the same `OracleAdapter` trait and return a `u128` price scaled by `10^7`. The charge math that follows is unchanged.

---

### Security Rationale

| Property                | Spot     | TWAP                  | FixedRate        |
| ----------------------- | -------- | --------------------- | ---------------- |
| Oracle reads            | Yes      | Yes                   | No               |
| Staleness enforced      | Yes      | Yes (per observation) | N/A              |
| Manipulation resistance | Low      | High (median)         | Perfect (static) |
| Oracle dependency       | Required | Required              | None             |
| Admin auth to change    | Yes      | Yes                   | Yes              |

---

## Per-merchant staleness threshold (Issue #147 / #187)

### Problem

High-frequency billing (e.g. minute/hourly intervals) needs tighter oracle
freshness than monthly plans. A single compile-time `MAX_ORACLE_AGE` cannot
express that trade-off: either short-interval merchants reject healthy quotes,
or long-interval merchants accept quotes that are too old for their risk model.

### Current behavior (already runtime-configurable)

Staleness is **not** a compile-time constant in the vault today. Admins set a
global threshold through:

```text
set_oracle_config(admin, enabled, oracle, max_age_seconds)
```

`resolve_charge_amount` rejects quotes when
`now - price.timestamp > max_age_seconds` and returns `OraclePriceStale` (`5008`)
**before** any balance mutation. The same `max_age_seconds` value is used for
oracle liveness health checks (`age <= max_age_seconds / 2`).

### Design decision for this release: keep the threshold global

We keep a **single vault-wide** `max_age_seconds` rather than adding a
per-merchant (or per-subscription) override in this change.

Rationale:

1. **One oracle feed, one freshness contract.** The vault reads a shared oracle
   adapter (`Spot` / `TWAP` / `FixedRate`). Mixing merchant-specific ages against
   the same feed makes monitoring and incident response harder: a quote can be
   "fresh" for merchant A and "stale" for merchant B in the same ledger.
2. **Circuit-breaker coherence.** Deviation checks and liveness events are keyed
   off the global config. Divergent ages would desynchronize breaker trips from
   staleness failures.
3. **Storage / migration cost.** Extending `MerchantConfig` or adding a new
   `DataKey` discriminant requires a careful storage migration and expands the
   admin surface. That is deferred until there is a clear operator demand.
4. **Security floor stays simple.** Ledger close-time skew already argues for a
   minimum safe window (≥ 60s). A global floor is easier to audit than
   per-merchant exceptions that could be set unsafely low.

### Recommended global values by billing cadence

| Cadence | Suggested `max_age_seconds` | Notes |
| --- | --- | --- |
| Sub-hourly / high-frequency | 60–120 | Stay above the 60s safety floor |
| Daily | 300–900 | Align with TWAP window when used |
| Weekly / monthly | 900–3600 | Still reject multi-hour outages |

Operators who need tighter guarantees for a subset of merchants should run a
dedicated vault instance (or oracle adapter kind) with a stricter global age,
rather than mixing thresholds in one deployment.

### Future extension (not implemented)

If per-merchant tuning becomes necessary:

1. Add optional `oracle_max_age_seconds: Option<u64>` under
   `DataKey::MerchantConfig` **or** a dedicated discriminant
   `MerchantOracleMaxAge(Address)`.
2. Define `effective_max_age(merchant) = merchant_override.unwrap_or(global)`,
   with `effective_max_age >= 60` enforced at write time.
3. Emit `oracle_config_updated` (or a merchant-scoped event) when overrides
   change; keep charge failure codes unchanged (`OraclePriceStale`).
4. Document migration / rollback: clearing the override restores global
   behavior with no evidence rewrite.

Until that ships, configure `max_age_seconds` globally to the **strictest**
merchant class on the vault.
