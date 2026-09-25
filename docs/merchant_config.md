# Merchant Configuration Storage

`SubscriptionVault` supports per-merchant configuration records to control subscription defaults, payout settings, and operational flags.

## Overview

The merchant configuration module provides:
- **Payout settings**: Address where merchant receives earnings
- **Fee configuration**: Fee percentage in bips (0-100%)
- **Operational flags**: Bitmap controlling allowed operations
- **Configurable pausing**: Per-merchant pause capability

## Data Structure

```rust
pub struct MerchantConfig {
    /// Schema version for forward-compatible upgrades
    pub version: i32,
    /// Address where merchant receives payouts
    pub payout_address: Address,
    /// Fee percentage in bips (0-10000, where 10000 = 100%)
    pub fee_bips: i32,
    /// Bitmap of allowed operations (see OP_* constants)
    pub allowed_operations: i32,
    /// Whether merchant can receive charges and payouts
    pub is_active: bool,
    /// Optional address for platform fee routing
    pub fee_address: Option<Address>,
    /// Redirect URL for off-chain callbacks
    pub redirect_url: String,
    /// Global pause for merchant subscriptions
    pub is_paused: bool,
    /// Timestamp of last config update
    pub last_updated: u64,
}
```

## Operation Flags

| Constant | Value | Description |
|---------|-------|-------------|
| `OP_CHARGE` | 0x01 (1<<0) | Can charge subscribers |
| `OP_WITHDRAW` | 0x02 (1<<1) | Can withdraw earnings |
| `OP_REFUND` | 0x04 (1<<2) | Can issue refunds |
| `OP_BILLING_PAUSE` | 0x08 (1<<3) | Can pause subscriptions globally |
| `OP_AUTO_RENEWAL` | 0x10 (1<<4) | Auto-renewal enabled |

Default: `OP_CHARGE | OP_WITHDRAW | OP_REFUND | OP_AUTO_RENEWAL`

## Constants

- `MAX_FEE_BIPS`: 10000 (100%)
- `DEFAULT_ALLOWED_OPS`: All operations except OP_BILLING_PAUSE

## Storage

Configs are stored under `DataKey::MerchantConfig(Address)` in instance storage.

## Entry Points

### initialize_merchant_config

Creates a new merchant config with validation.

```rust
pub fn initialize_merchant_config(
    env: Env,
    merchant: Address,           // Must authorize
    payout_address: Address,     // Where merchant receives payouts
    fee_bips: i32,              // Fee in bips (0-10000)
    allowed_operations: i32,    // Operation bitmap
    fee_address: Option<Address>,
    redirect_url: String,
) -> Result<MerchantConfig, Error>
```

Errors:
- `InvalidFeeBips` - fee exceeds 100%
- `InvalidOperations` - invalid operation bits
- `MustAllowChargeOperation` - CHARGE must be enabled

### set_merchant_config

Full overwrite with validation.

```rust
pub fn set_merchant_config(
    env: Env,
    merchant: Address,
    config: MerchantConfig,
) -> Result<(), Error>
```

### update_merchant_config

Partial update - `None` leaves fields unchanged.

```rust
pub fn update_merchant_config(
    env: Env,
    merchant: Address,
    new_payout_address: Option<Address>,
    new_fee_bips: Option<i32>,
    new_allowed_operations: Option<i32>,
    new_is_active: Option<bool>,
    new_fee_address: Option<Option<Address>>,
    new_redirect_url: Option<String>,
    new_is_paused: Option<bool>,
) -> Result<MerchantConfig, Error>
```

### get_merchant_config

Query configuration.

```rust
pub fn get_merchant_config(
    env: Env,
    merchant: Address,
) -> Option<MerchantConfig>
```

## Field mutability while subscriptions are active

Every field of `MerchantConfig` can be changed at any time by the merchant
itself. `update_merchant_config` applies each `Some(..)` field independently
and **performs no check on how many subscriptions the merchant currently has**.
This section states, per field, what changing it actually affects, so a
merchant does not discover the consequences by surprise.

> **Known gap.** The issue for this section also asked that protected fields
> return `Error::InvalidStatusTransition` when changed while active
> subscriptions exist. **That guard does not exist in the contract today** —
> `update_merchant_config` in `merchant.rs` validates field *shape* only
> (`fee_bips <= MAX_FEE_BIPS`, `OP_CHARGE` must be set) and never inspects the
> merchant's subscription set. It is documented here as a gap rather than
> described as if it were enforced. See [Known gaps](#known-gaps).

### Mutability matrix

| Field | Mutable while active? | Effect of changing it mid-flight | Enforced today |
|-------|:---------------------:|-----------------------------------|----------------|
| `payout_address` | Yes | **Immediate and retroactive in effect.** All future payouts, and any not-yet-withdrawn `TokenEarnings`, go to the new address. Funds already withdrawn are unaffected. | No subscription guard |
| `fee_bips` | Yes | **Applies to future charges only.** Already-executed statements and already-accrued `MerchantEarnings` keep the old rate. In-flight charges take the rate at execution time. | Range-checked: `> MAX_FEE_BIPS` → `InvalidFeeBips` |
| `fee_address` | Yes | Fee share is paid to this address instead of the payout address going forward. `None` means "use `payout_address`". | No guard |
| `allowed_operations` | Yes | Operation bitmap is read per operation. Clearing a bit blocks that operation from the next call onward. `CHARGE` **cannot** be cleared. | Validated: unknown bits → `InvalidOperations`; `CHARGE` unset → `MustAllowChargeOperation` |
| `is_active` | Yes | Merchants can self-deactivate. Blocks new subscription creation for this merchant; does **not** stop existing subscriptions from being charged. | No guard |
| `is_paused` | Yes | Blocks charges to this merchant's subscriptions, returning `Error::MerchantPaused`. Does not block withdrawal of already-earned funds. | No guard |
| `redirect_url` | Yes | Off-chain checkout/redirect hint only. No on-chain financial effect. | No guard |
| `version` | Not client-settable | Schema version, maintained by the contract. | Written internally |
| `last_updated` | Not client-settable | Set to `env.ledger().timestamp()` on every successful update. | Written internally |

### Which fields are economically significant

Four fields change who gets money or whether money moves at all, and are the
ones to treat as high-risk while subscriptions are live:

| Field | Why it is sensitive | Safe pattern |
|-------|--------------------|---------------|
| `payout_address` | Redirects the entire payout stream. A compromised or mistyped address sends real funds to a third party, and there is no clawback. | Rotate only immediately after a key compromise, or after draining `TokenEarnings` to the old address. Verify the address on-chain before signing. |
| `fee_address` | Same redirection risk for the fee share. | Same as `payout_address`. |
| `fee_bips` | Changes merchant revenue on every future charge. Raising it while subscriptions are active retroactively worsens terms subscribers agreed to. | Announce before raising. Consider pausing new subscriptions first. |
| `is_paused` | Halts all charges for the merchant. Use it to stop an active incident. | This is the intended incident control, and is safe: subscribers can still cancel and withdraw. |

### Fields that are safe to change at any time

`redirect_url` has no on-chain financial effect and `allowed_operations` only
takes effect on the next operation, provided `CHARGE` is retained. Neither
requires pausing.

### Ordering rule for sensitive changes

Because no field is blocked while subscriptions are active, the safe order for
a `payout_address` rotation is:

1. Optionally drain accrued earnings to the **old** address first, so the
   change only redirects future accrual.
2. `update_merchant_config(merchant, new_payout_address = Some(new), ..)`.
3. Confirm the `merchant_config_updated` event and re-read `get_merchant_config`.

Setting `is_paused = true` first is the conservative alternative: it halts new
charges, lets you make the change without fresh money moving, and does not
trap funds (withdrawals remain available).

### Known gaps

The following are **not** enforced by the contract and are recorded here so
that integrators do not rely on behaviour that does not exist:

1. **No active-subscription guard.** `update_merchant_config` does not check
   the merchant's subscription count, so `payout_address`, `fee_address`,
   `fee_bips`, and `is_active` can all be changed while charges are in flight.
   `Error::InvalidStatusTransition` is **not** returned in any of these cases.
2. **No `payout_address` history.** The previous payout address is not retained
   on-chain; `MerchantConfigUpdatedEvent` only carries the *new* value. Off-chain
   indexers are the sole record of the old address.
3. **`is_active` vs `is_paused` are independent.** Setting `is_active = false`
   does not pause existing subscriptions, and `is_paused = true` does not
   deactivate the merchant. Both may need to be set to fully stop a merchant.

Closing gap 1 would mean rejecting `update_merchant_config` when the merchant
has any subscription in a non-terminal status. That is a behaviour change to a
live, merchant-authorized entry point and needs its own review and test
coverage rather than a documentation-only patch.

## Validation Functions

```rust
pub fn is_valid_allowed_operations(ops: i32) -> bool
```

Validates:
- Only valid operation bits are set
- OP_CHARGE is enabled (required for operation)

## Events

### MerchantConfigInitializedEvent

```rust
pub struct MerchantConfigInitializedEvent {
    pub merchant: Address,
    pub payout_address: Address,
    pub fee_bips: i32,
    pub allowed_operations: i32,
    pub timestamp: u64,
}
```

### MerchantConfigUpdatedEvent

```rust
pub struct MerchantConfigUpdatedEvent {
    pub merchant: Address,
    pub payout_address: Address,
    pub fee_bips: i32,
    pub allowed_operations: i32,
    pub is_active: bool,
    pub timestamp: u64,
}
```

## Security Assumptions

1. **Merchant authorization**: Only merchant can modify their own config
2. **Fee validation**: Fee cannot exceed 100% (10000 bips)
3. **Operation validation**: CHARGE operation must be enabled
4. **Payout safety**: Payout address validation is delegated to caller context
5. **Event auditability**: All config changes emit events for indexers

## Storage Schema

```text
DataKey::MerchantConfig(Address) => MerchantConfig
```

## Usage Examples

### Initialize merchant config
```rust
let config = client.initialize_merchant_config(
    &merchant,           // authorized signer
    payout_address,
    500,                 // 5% fee (500 bips)
    0x1F,                // all operations enabled
    None,                // no fee routing
    String::from_str(&env, "https://example.com/callback"),
)?;
```

### Query config
```rust
let config = client.get_merchant_config(&merchant);
match config {
    Some(c) => assert!(c.is_active),
    None => panic!("merchant not initialized"),
}
```

### Update specific field
```rust
let updated = client.update_merchant_config(
    &merchant,
    None,           // payout unchanged
    Some(1000),     // update fee to 10%
    None,           // operations unchanged
    None,           // active unchanged
    None,           // fee_address unchanged
    None,           // redirect unchanged
    None,           // paused unchanged
)?;
```

## Upgradability

The config struct includes `version` field for forward-compatible upgrades. New fields can be added with migration logic while preserving existing storage layout.