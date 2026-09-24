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
