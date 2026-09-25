# `batch_charge`

Charges multiple subscriptions in one transaction. The entrypoint is
admin-only and is guarded by the emergency stop flag.

## Signature
```rust
pub fn batch_charge(
	env: Env,
	subscription_ids: Vec<u32>,
	nonce: u64,
) -> Result<Vec<BatchChargeResult>, Error>
```

## Maximum batch size

`batch_charge` accepts **at most `BATCH_MAX_SIZE` (100) subscription IDs per
call.** This bound is a hard constant, not an advisory guideline:

| Constant | Value | Defined in |
|----------|-------|------------|
| `BATCH_MAX_SIZE` | `100` | `contracts/subscription_vault/src/types.rs` |

The same constant bounds every other bulk entrypoint — `bulk_pause`,
`bulk_cancel`, and the admin bulk operations — so `100` is the maximum batch
size for any vector of subscription IDs the contract accepts.

### Oversized batches are rejected cleanly

If `subscription_ids.len() > BATCH_MAX_SIZE`, the call **fails as a whole**
before any subscription is touched:

```
Error::BatchTooLarge   // code 1006
```

This is a typed, catchable contract error rather than a generic execution
failure. Without this guard, an oversized batch would instead exhaust the
Soroban per-transaction instruction budget and abort with an opaque host
error that integrators cannot distinguish from a network or resource problem.

> **Note on error taxonomy:** `BatchTooLarge` (1006) sits in the `1000-1099`
> **auth** range but is semantically an input-validation failure, in the same
> family as `InvalidInput` (3002). Both are caller-fixable: the caller must
> split the batch. Integrators that only special-case `InvalidInput` should
> also treat `BatchTooLarge` as a "fix the request" signal and split the input
> into chunks of at most `100` IDs. See
> [`docs/errors.md`](errors.md#canonical-table) for the canonical table.

### Check ordering — a rejected batch never burns a nonce

The size check runs in the shared `bulk_precheck` helper **before** the batch
nonce is consumed:

1. `require_admin_or_operator_auth` — caller must be admin or operator.
2. `ids.len() > BATCH_MAX_SIZE` → `Err(Error::BatchTooLarge)`.
3. `ids.is_empty()` → no-op (`Ok(false)`), no nonce consumed, no event.
4. `check_and_advance(nonce)` — consume the per-batch nonce.

Because the length check precedes step 4, a rejected oversized batch leaves
the caller's nonce sequence **untouched**. The caller may correct the input
and retry with the *same* nonce value; it does not have to burn a nonce on a
request that never did any work.

### Integrator guidance

- Chunk large billing runs into groups of `<= 100` IDs.
- Treat `BatchTooLarge` as terminal for that request shape — retrying the
  identical oversized vector will fail identically.
- Each chunk is an independent transaction, so chunk boundaries are also the
  natural place to checkpoint progress off-chain.

## Partial-success model

Admin authentication and the batch nonce check happen once at the batch
boundary. After those checks pass, each ID is processed independently through
the shared `charge_one` path. An item failure does not roll back successful
items or abort the remaining items. The call returns exactly one result for
each input ID, including IDs that are missing or repeated.

## `BatchChargeResult`

The result vector has the same order and length as `subscription_ids`:

| Field | Type | Values |
|-------|------|--------|
| `success` | `bool` | `true` when that item completed without an item-level error |
| `error_code` | `u32` | `0` on success; otherwise the corresponding `Error` code |

Successful interval charges mutate only their own subscription and accounting
state. Failed items return their error code and retain the single-charge
semantics, including any per-item lifecycle transition such as entering
`GracePeriod` or `InsufficientBalance`.

## Ordering guarantees

Results are appended while iterating `subscription_ids`, so result `i`
corresponds to input ID `i`. Processing is sequential. This means duplicate
IDs are processed at each position, and a later duplicate observes state
written by the earlier occurrence in the same batch.

## Skip conditions
- Subscription not found
- Status is Paused, Cancelled, or InsufficientBalance
- Billing interval has not elapsed
- Insufficient prepaid balance (also applies the lifecycle grace rule)

## Retry guidance

The batch nonce is consumed before item processing. A retry of the same batch
must therefore use a fresh nonce; reusing the old nonce is rejected as a
replay. Prefer retrying only the failed IDs after correcting their cause:
top up and explicitly resume an underfunded subscription, wait for an interval
that has not elapsed, or remove/replace an ID that no longer exists. Do not
blindly retry successful IDs, because their next result may be
`IntervalNotElapsed` or another state-dependent outcome.

The outer `Result` is an error rather than a result vector when the emergency
stop is active, authentication fails, or the batch nonce is invalid. In
particular, enabling the emergency stop prevents any item from being charged;
it does not produce partial per-item results. Pause, resume, cancel, and query
operations remain available according to the emergency-stop policy.
