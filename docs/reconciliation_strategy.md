# Billing Statement Reconciliation Strategy

When billing statements are pruned (compacted), the detailed history is replaced by a `BillingStatementAggregate`. To maintain financial reporting accuracy and perform reconciliation, follow this strategy.

## 1. Aggregate Structure
The `BillingStatementAggregate` stores the summary of all pruned statements:
- `pruned_count`: Total number of rows removed.
- `total_amount`: Sum of `amount` across all removed rows.
- `totals`: Per-kind breakdown (`interval`, `usage`, `one_off`).
- `oldest_period_start` / `newest_period_end`: Time range covered by pruned data.

## 2. Reconstructing Full History
To calculate the total billed amount for a subscription since its creation:
```
Total Billed = Aggregate.total_amount + Sum(LiveStatements.amount)
```

To calculate the breakdown per charge kind:
```
Total Interval = Aggregate.totals.interval + Sum(LiveStatements where kind == Interval)
Total Usage = Aggregate.totals.usage + Sum(LiveStatements where kind == Usage)
Total One-Off = Aggregate.totals.one_off + Sum(LiveStatements where kind == OneOff)
```

## 3. Verification & Integrity
- **Sequence Integrity**: The lowest `sequence` number in the live statements should be equal to `Aggregate.pruned_count`. Any gap indicates a data integrity issue.
- **Count Consistency**: `SubscriptionVault::get_total_statements` returns the count of *live* statements. The total number of statements ever created can be inferred as `Aggregate.pruned_count + LiveCount`.
- **Amount Consistency**: The `Subscription::lifetime_charged` field should always equal the sum of all billing statements (compacted + live).
    - Note: Differences may arise if refunds were processed, which are tracked separately in `MerchantEarnings`.

## 4. Reconciliation Workflow
1. Call `get_stmt_compacted_aggregate(subscription_id)` to get the summary of pruned history.
2. Call `get_sub_statements_offset` or `get_sub_statements_cursor` to fetch live detailed rows.
3. Sum the values as described above.
4. Compare against `get_subscription(subscription_id).lifetime_charged` for high-level validation.

---

# Contract-Level Reconciliation Queries

The contract provides read-only endpoints for off-chain auditors to validate the accounting equation:

```
contract_token_balance = total_prepaid + total_merchant_liabilities + recoverable
```

## API Overview

### 1. Token-Level Reconciliation: `get_token_reconciliation(token)`

Returns complete reconciliation data for a single settlement token.

**Response: `TokenLiabilities`**
- `token`: Token contract address
- `total_prepaid`: Sum of all subscriber prepaid balances
- `total_merchant_liabilities`: Sum of all merchant earnings (accruals - withdrawals - refunds)
- `recoverable_amount`: Stranded funds that can be recovered by admin
- `contract_balance`: Actual token balance held by the contract
- `computed_total`: Prepaid + merchant liabilities + recoverable
- `is_balanced`: Whether the accounting equation validates

**Usage:**
```rust
let reconciliation = client.get_token_reconciliation(&usdc_token);
assert!(reconciliation.is_balanced);
assert_eq!(
    reconciliation.contract_balance,
    reconciliation.total_prepaid
        + reconciliation.total_merchant_liabilities
        + reconciliation.recoverable_amount
);
```

### 2. Multi-Token Summary: `get_contract_reconciliation_summary(start_token_index, limit)`

Returns paginated reconciliation data for all accepted tokens.

**Parameters:**
- `start_token_index`: Index into accepted tokens list (0 for first page)
- `limit`: Maximum summaries to return (capped at 50)

**Response: `ReconciliationSummaryPage`**
- `token_summaries`: Vector of `TokenLiabilities`
- `next_token_index`: Cursor for next page, `None` when complete

**Usage:**
```rust
// Get all token reconciliations
let mut index = 0u32;
loop {
    let page = client.get_contract_reconciliation_summary(&index, &50);
    for summary in &page.token_summaries {
        println!("Token: {:?}, Balanced: {}", summary.token, summary.is_balanced);
    }
    match page.next_token_index {
        Some(next) => index = next,
        None => break,
    }
}
```

### 3. Auditable Proof Generation: `generate_reconciliation_proof(token)`

Creates an auditable snapshot with all data needed to independently validate the accounting equation.

**Response: `ReconciliationProof`**
- `timestamp`: Ledger timestamp when proof was generated
- `ledger_sequence`: Ledger sequence for temporal anchoring
- `token`: Token being audited
- `contract_balance`: Contract's token balance
- `total_prepaid`: Sum of all subscriber prepaid balances
- `total_merchant_liabilities`: Total merchant earnings liabilities
- `computed_recoverable`: Calculated recoverable amount
- `subscription_count`: Number of subscriptions scanned
- `merchant_count`: Number of merchants with earnings
- `is_valid`: Whether the accounting equation validates

**Security Properties:**
- Read-only: Cannot modify contract state
- Temporally anchored: Includes ledger sequence
- Self-contained: All validation data in one struct

### 4. Paginated Prepaid Query: `query_prepaid_balances_paginated(request)`

Bounded-compute query for aggregating prepaid balances across subscriptions.

**Request: `PrepaidQueryRequest`**
- `token`: Token to filter by (required)
- `start_subscription_id`: Starting subscription ID (inclusive)
- `scan_limit`: Maximum subscriptions to scan (capped at 500)

**Response: `PrepaidQueryResult`**
- `token`: Token queried
- `partial_total`: Sum of prepaid balances in scan window
- `subscriptions_count`: Number of subscriptions with non-zero prepaid
- `next_start_id`: Next ID to scan, `None` if complete
- `has_more`: Whether more subscriptions exist beyond window

**Off-Chain Aggregation Example:**
```rust
let mut total_prepaid = 0i128;
let mut start_id = 0u32;

loop {
    let result = client.query_prepaid_balances_paginated(&PrepaidQueryRequest {
        token: usdc_token.clone(),
        start_subscription_id: start_id,
        scan_limit: 500,
    });

    total_prepaid += result.partial_total;

    if !result.has_more {
        break;
    }
    start_id = result.next_start_id.unwrap();
}
```

## Reconciliation Workflow for Auditors

### Quick Validation (Single Token)
```rust
// 1. Get reconciliation data
let recon = client.get_token_reconciliation(&token);

// 2. Verify accounting equation
assert!(recon.is_balanced, "Accounting equation does not balance!");

// 3. Verify specific amounts
assert_eq!(
    recon.contract_balance,
    recon.total_prepaid + recon.total_merchant_liabilities + recon.recoverable_amount
);
```

### Full Audit with Proof Generation
```rust
// Generate proof for record keeping
let proof = client.generate_reconciliation_proof(&token);

// Store proof off-chain with ledger sequence for temporal reference
store_audit_record(proof.ledger_sequence, proof);

// Verify at a later date
let current = client.get_token_reconciliation(&token);
assert_eq!(current.contract_balance, proof.contract_balance); // Or investigate changes
```

### Multi-Token Portfolio Reconciliation
```rust
let mut all_balanced = true;
let mut start_index = 0u32;

loop {
    let page = client.get_contract_reconciliation_summary(&start_index, &50);

    for summary in &page.token_summaries {
        if !summary.is_balanced {
            all_balanced = false;
            log_imbalance(&summary.token, summary);
        }
    }

    match page.next_token_index {
        Some(next) => start_index = next,
        None => break,
    }
}

assert!(all_balanced, "Some tokens have accounting imbalances!");
```

## Performance & Security Considerations

### Bounded Compute
- `MAX_PREPAID_SCAN_DEPTH = 500`: Limits subscription scans per call
- `MAX_TOKEN_SUMMARIES_PER_PAGE = 50`: Limits token summaries per call
- Indexers should chain paginated calls to build complete totals

### Gas Efficiency
- `get_token_reconciliation`: O(subscriptions + merchants) — use for spot checks
- `generate_reconciliation_proof`: Same complexity but returns compact proof
- `query_prepaid_balances_paginated`: O(scan_limit) — bounded and predictable

### Read-Only Safety
All reconciliation endpoints are read-only and cannot modify contract state. They:
- Require no authentication
- Emit no events
- Have no side effects
- Are safe to call at any time

## Indexer Integration

Indexers computing off-chain proofs should:

1. **Use paginated queries** for large datasets
2. **Validate proofs** against on-chain data periodically
3. **Store ledger sequences** with proof records for temporal validation
4. **Monitor `is_balanced`** for anomaly detection
5. **Aggregate across pages** to verify total contract liabilities

Example indexer proof computation:
```rust
// 1. Collect paginated prepaid data
let prepaid_total = aggregate_paginated_prepaid(&client, &token);

// 2. Get merchant liabilities (from indexed data or contract)
let merchant_total = get_indexed_merchant_liabilities(&token);

// 3. Get contract balance from token contract
let contract_balance = token_client.balance(&vault_address);

// 4. Compute and verify
let recoverable = contract_balance - prepaid_total - merchant_total;
assert!(recoverable >= 0, "Negative recoverable indicates data inconsistency");
```

---

# 5. Post-Emergency-Stop Reconciliation Runbook

> Incident-response context: [`docs/emergency_stop.md`](emergency_stop.md).

An emergency stop halts all value-moving charge operations. When it is lifted,
the accounting state is **not** automatically repaired — no catch-up charges
are generated, and usage reported during the outage was never recorded. This
runbook covers the reconciliation that must be performed *before* the first
post-stop billing run, and the invariants that will otherwise look like bugs.

## 5.1 What the stop actually suspends

From `docs/emergency_stop.md`, the operations that fail with
`Error::EmergencyStopActive` (1009) while the stop is active are:

| Suspended | Not suspended |
|-----------|---------------|
| `charge_subscription`, `batch_charge` | `get_*` queries (all read paths) |
| `charge_usage`, `charge_usage_with_reference` | `cancel_subscription` |
| `charge_one_off` | `pause_subscription` / `resume_subscription` |
| `deposit_funds` | `withdraw_subscriber_funds` |
| `create_subscription*` | `withdraw_merchant_funds` |
| `partial_refund` | |

This asymmetry is the root of every reconciliation question that follows:
**reads and withdrawals keep working, so on-chain state stays internally
consistent, while accrual silently stops.**

## 5.2 Key invariant: missed intervals are NOT back-billed

This is the single most important thing to understand before reconciling.

Interval charges use **window reset** semantics, not catch-up semantics. On a
successful charge the contract sets:

```
last_payment_timestamp = env.ledger().timestamp()   // "now", not last + interval
```

Consequences after a stop of duration `D` on a subscription with interval `I`:

- The first post-stop charge **succeeds as soon as `D >= I`** and bills exactly
  **one** interval charge.
- The remaining `floor(D / I) - 1` missed intervals are **permanently
  forgiven**. They are not billed later and are not queued.
- `lifetime_charged` reflects only the charges that actually executed, so it
  will legitimately be lower than `elapsed_time / I * amount`.

**Reconciliation rule:** a post-stop gap between `lifetime_charged` and naive
elapsed-time expectation is **correct behaviour, not a discrepancy**. Do not
raise a reconciliation exception for it, and do not "fix" it with a manual
recovery — the funds were never owed. See
[`docs/billing_intervals.md`](billing_intervals.md#window-reset).

## 5.3 What accumulates during the stop

| Quantity | Behaviour during stop | Post-stop obligation |
|----------|----------------------|----------------------|
| `last_payment_timestamp` | Frozen (no charges execute) | Re-baselined to `now` on first success |
| `lifetime_charged` | Unchanged | Unchanged; no back-bill |
| Billing statements | None emitted | Gap is expected; do not synthesize rows |
| Merchant earnings | No accrual | Accrues only from post-stop charges |
| Protocol fees | No accrual | Accrues only from post-stop charges |
| Prepaid balance | Frozen; `deposit_funds` blocked | May be insufficient → see 5.5 |
| Usage counters | `charge_usage` calls **fail**, so caller-reported usage is lost | Re-submit buffered usage, see 5.4 |

Because no statements are written during the stop, the *contract-level*
accounting equation is **unbroken** throughout:

```
contract_token_balance = total_prepaid + total_merchant_liabilities + recoverable
```

`recoverable` will *not* drift upward due to the stop. If it does, the cause is
unrelated to the incident and must be investigated as a genuine anomaly.

## 5.4 Runbook — procedure

Perform in order. Steps 1-3 are read-only and safe to run at any time.

**Step 0 — Record the incident window.**

```bash
# From the event stream; both events are emitted by admin transitions.
ENABLED_LEDGER  = <ledger of EmergencyStopEnabledEvent>
DISABLED_LEDGER = <ledger of EmergencyStopDisabledEvent>
STOP_START_TS   = <timestamp in ENABLED_LEDGER>
STOP_END_TS     = <timestamp in DISABLED_LEDGER>
STOP_DURATION   = STOP_END_TS - STOP_START_TS
```

Keep these values: every subsequent check is parameterised by them.

**Step 1 — Baseline the accounting equation before touching anything.**

```bash
# Per settlement token, immediately after the stop is lifted.
get_token_reconciliation(token)      # total_prepaid, total_merchant_liabilities,
                                     # recoverable, is_balanced
```

Record `total_prepaid` and `total_merchant_liabilities` as `BASELINE_*`. The
invariant `recoverable == contract_balance - total_prepaid - merchant_liabilities`
must hold exactly, and `is_balanced` must be true, **before** the first billing
run. If it does not, the incident is not the cause — stop and escalate.

**Step 2 — Verify the stop lifted cleanly.**

```bash
get_emergency_stop_status()    # must be false
```

Also confirm exactly one `EmergencyStopEnabledEvent` and one
`EmergencyStopDisabledEvent` in the window. A missing or duplicated pair means
the incident timeline is unreliable and the rest of this runbook cannot be
trusted.

**Step 3 — Snapshot affected subscriptions.**

Identify every subscription that was `Active` at `STOP_START_TS` — those are
the ones whose accrual was interrupted. For each, capture `id`, `status`,
`last_payment_timestamp`, `lifetime_charged`, `prepaid_balance`, `token`,
`interval_seconds`, and whether usage limits are configured.

```bash
get_subscription(id)
```

**Step 4 — Classify each subscription against the stop duration.**

Let `remaining = STOP_END_TS - last_payment_timestamp` at the snapshot.

| Condition | Classification | Action |
|-----------|----------------|--------|
| `remaining < interval_seconds` | Not yet due | Skip. Not chargeable; no action. |
| `remaining >= interval_seconds` and `prepaid_balance >= amount` | Chargeable | Include in first billing run |
| `remaining >= interval_seconds` and `prepaid_balance < amount` | Underfunded | Do **not** bulk-charge. See 5.5 |
| `status != Active` (Paused / Cancelled / GracePeriod / InsufficientBalance) | Inactive | Excluded from the run; resumes on its own lifecycle event |

`remaining < interval_seconds` is the common case for short incidents and is
entirely benign.

**Step 5 — Handle buffered usage (usage-metered subscriptions only).**

Usage is **reported per call**, not accrued by the contract. Every
`charge_usage` / `charge_usage_with_reference` attempted during the stop
failed with `EmergencyStopActive`, so that usage never reached storage. The
metering pipeline must hold those amounts and re-submit after the lift.

Two post-stop hazards specific to usage:

- **Burst-limit rejection on the first resubmission.** Burst protection
  compares `now - last_usage_timestamp` against `burst_min_interval_secs`.
  After a long stop this gap is large and passes. After a *short* stop it can
  be smaller than the burst window, and the first resubmitted call is rejected
  with `BurstLimitExceeded`. Back off and retry after the burst window.
- **Per-interval cap reset.** `current_period = (now - start_time) /
  interval_seconds`; when the period advances, `current_period_usage_units`
  resets to `0`. A stop spanning more than one interval therefore *resets* the
  cap, and usage counted in the pre-stop period is no longer counted against
  the cap. Do not assume the cap spans the outage.

Resubmit buffered usage with **fresh** `reference` strings. Reusing a
reference returns `Replay` (1102) — and because the original call failed, no
usage was recorded under it, so a fresh reference is correct and necessary.

**Step 6 — Run the first billing batch.**

```bash
batch_charge([chargeable_ids], fresh_nonce)     # <= 100 ids per call
```

Use a **fresh nonce** — the previous run's nonce is consumed. Parse every
`BatchChargeResult`; partial failures are expected and are handled per
`docs/integration_guide.md`. Expect `IntervalNotElapsed` for any subscription
whose `remaining` shrank between step 4 and execution.

**Step 7 — Post-run verification.**

Re-run step 1 and confirm:

- The accounting equation still balances and `is_balanced` is still true.
- `total_prepaid` decreased by roughly the sum of successful charges.
- `total_merchant_liabilities` increased by the merchant share of those charges.
- No subscription shows `lifetime_charged` growth beyond one interval per
  successful charge. **Growth beyond that is the real anomaly** — it would
  indicate catch-up billing that should not be possible.

**Step 8 — Close the incident.**

- Record `STOP_START_TS`, `STOP_END_TS`, the step 1 baseline, and the
  post-run figures in the incident report.
- Note explicitly that missed intervals were forgiven by design (5.2) and
  usage during the outage was resubmitted or dropped (5.5 / 5.4). Subscribers
  who expect pro-rata catch-up billing must be told this is by design.
- Reconcile any **fee shortfall** deliberately: protocol fees and merchant
  earnings did not accrue during the stop. Report the forgone amount as an
  incident cost; do not attempt to recover it from subscribers, because the
  contract has no mechanism to bill retroactively and manual recovery would
  reach into subscriber prepaid balances (prohibited — see
  [`docs/recovery.md`](recovery.md), "Recovery vs. cancellation").

## 5.5 Subscribers who could not top up

`deposit_funds` is suspended during the stop, so subscribers had no way to
maintain their balance. After the lift:

- Subscriptions with sufficient balance charge normally.
- Subscriptions with insufficient balance **will not be force-charged**. A
  charge that would overdraw is refused; the subscription follows its normal
  lifecycle path toward `GracePeriod` and then `InsufficientBalance`.
- Re-enable deposits, then wait for `RecoveryReadyEvent` /
  `SubscriptionResumedEvent` before retrying. Do not retry on a timer — a
  tight retry loop against an underfunded subscription produces repeated
  failed results with no progress.

## 5.6 Post-run checklist

- [ ] `is_balanced` true for every settlement token
- [ ] `get_emergency_stop_status()` false
- [ ] Exactly one enable + one disable event in the window
- [ ] Every step-4 `Chargeable` id either charged or has a recorded reason
- [ ] Buffered usage resubmitted with fresh references, or explicitly written off
- [ ] No subscription billed more than one interval
- [ ] Baseline and post-run figures recorded in the incident report
- [ ] Forgiven intervals and fee shortfall disclosed to affected parties

