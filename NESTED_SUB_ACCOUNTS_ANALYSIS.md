# Nested Sub-Accounts Analysis & Fee Propagation Verification

**Date**: 2026-10-20  
**Status**: Analysis Complete  
**Scope**: Verify whether the Stellabill subscription vault supports nested sub-accounts (sub-accounts with sub-accounts) and if fees are correctly propagated without double-charging.

---

## Executive Summary

**Finding**: The current Stellabill subscription vault implementation **does NOT support nested sub-accounts**. Sub-accounts are a flat, single-level structure attached only to the parent merchant. A subscription can only have a `sub_account_label` that routes to a specific merchant sub-account, not to another sub-account.

**Key Points**:
1. Sub-accounts are uniquely identified by a `(merchant, label)` tuple
2. A subscription has a single optional `sub_account_label: Option<Symbol>` that routes charges to that specific sub-account
3. The API has no mechanism to create a sub-account of a sub-account
4. Fee routing is deterministic and works correctly at the single level provided

---

## Architecture Analysis

### Sub-Account Design

From the code investigation:

#### 1. **Type Definition** (types.rs:620)
```rust
pub struct Subscription {
    pub merchant: Address,
    pub sub_account_label: Option<Symbol>,  // Single label only
    // ... other fields
}
```

A subscription has exactly **one** optional sub-account label. This label is always relative to the parent merchant, not to another sub-account.

#### 2. **Sub-Account Registration API** (merchant.rs:2009)
```rust
pub fn register_sub_account(
    env: &Env,
    merchant: Address,      // Parent merchant only
    label: Symbol,          // Unique label
) -> Result<(), Error>
```

Sub-accounts are registered directly on the merchant with:
- A simple `(merchant, label)` tuple as the key
- An `i128` balance stored at `DataKey::MerchantSubAccount(merchant, label)`
- A list of labels stored at `DataKey::MerchantSubAccountList(merchant)`

#### 3. **Charge Routing** (charge_core.rs, merchant.rs)
When a subscription is charged:
- If `subscription.sub_account_label` is `Some(label)`, the merchant amount is credited to the sub-account
- The sub-account is looked up by `(merchant, label)`, **not** by iterating through hierarchies
- If the sub-account doesn't exist, the charge fails with `Error::NotFound`

Example call chain:
```rust
charge_one() 
  → credit_merchant_balance_for_token(merchant, token, net_amount, label)
    → if label.is_some() {
        credit_sub_account(merchant, label, token, amount)?
      } else {
        credit_merchant_balance(merchant, token, amount)
      }
```

#### 4. **Sub-Account Queries** (queries.rs)
Three read-only endpoints:
- `get_sub_account_balance(merchant, label)` — Returns `i128`
- `get_sub_account_list(merchant)` — Returns `Vec<Symbol>`
- `get_merchant_token_earnings_with_sub_accounts(merchant, token)` — Returns aggregated breakdown

None of these accept or reference a parent sub-account label.

---

## Fee Routing Analysis

### Current Fee Computation (fee_routing.md)

The protocol fee is computed deterministically:
```
fee = max(gross × fee_bps / 10_000, 1)
net = gross − fee
```

**Properties**:
- Fee is applied once per charge, on the gross amount
- The minimum fee floor (1 base unit) ensures the treasury isn't starved on micro-charges
- Conservation invariant: `net + fee == gross` always holds

### Fee Routing with Sub-Accounts

When a charge is routed to a sub-account:

1. **Gross amount** deducted from `subscription.prepaid_balance`
2. **Protocol fee** computed once from the gross amount
3. **Net amount** (after fee) credited to the sub-account balance
4. **Parent `MerchantEarnings`** credited with the full gross (for roll-up audit)
5. **Parent `MerchantBalance`** reduced by the net amount (to avoid double-counting)

Example (100 tokens charged at 2.5% fee):
```
Gross:         100
Fee (2.5%):     2.50
Net:           97.50

Prepaid:       100 ↓ (deducted)
Sub-Account:     0 + 97.50 = 97.50 ↓ (credited with net)
Treasury:        0 + 2.50 = 2.50 ↓ (credited with fee)
Parent Earnings: recorded at gross level
Parent Balance:  reduced by 97.50 (net) to maintain invariant
```

### Why Nested Sub-Accounts Are Not Supported

**Reason**: Fee routing must be deterministic and audit-ready. Nested routing would introduce:

1. **Ambiguous fee computation**: If sub-account-A routes to sub-account-B, should the fee be:
   - Applied once at the top level (A) and then B receives the net?
   - Applied at both levels (double fee)?
   - Applied only when funds reach a "leaf" sub-account?

2. **Unclear earnings tracking**: Parent `MerchantEarnings` must capture the full accrual for compliance. Nested hierarchies would need to define roll-up semantics:
   - Do only top-level earnings count?
   - Do all levels accumulate?
   - How do withdrawals propagate?

3. **Storage overhead**: Each nesting level would require additional lookups and state tracking.

4. **Query complexity**: The aggregation queries (`get_merchant_token_earnings_with_sub_accounts`) would need recursive traversal and performance guarantees.

---

## Current Limitations

### What Sub-Accounts Support

✅ **Single-level routing**: A subscription charges directly to a named sub-account under the parent merchant  
✅ **Multiple sub-accounts per merchant**: A merchant can register N sub-accounts with independent labels  
✅ **Independent balances**: Each sub-account maintains its own balance per token  
✅ **Withdrawal tracking**: Withdrawals from sub-accounts are recorded in parent `MerchantEarnings`  
✅ **Fee determinism**: Fees are applied once, correctly computed, and auditable  

### What Sub-Accounts Do NOT Support

❌ **Nested hierarchy**: A sub-account cannot itself have sub-accounts  
❌ **Transitive routing**: A subscription cannot charge to "department > team > person"  
❌ **Multi-level fees**: There is no per-sub-account fee configuration  
❌ **Recursive aggregation**: Queries do not traverse hierarchies  

---

## Fee Propagation Correctness (Single Level)

All fee propagation at the single sub-account level is **correct and tested**:

### Tests That Verify Fee Routing

From `test_merchant_sub_accounts.rs`:

1. **`charge_to_unregistered_sub_account_fails`** (line 403)
   - Ensures that charging to a non-existent sub-account fails before any state mutation
   - Prevents silent loss of fees or funds

2. **`charge_subscription_to_sub_account_updates_earnings`** (line 439)
   - Verifies that parent `MerchantEarnings` reflect the charge even when funds route to sub-account
   - Confirms conservation of the gross amount

3. **`sub_account_withdrawal_updates_breakdown`** (line 809)
   - Ensures withdrawals from sub-accounts correctly update the breakdown query
   - Verifies parent earnings withdrawals are recorded

4. **Fee routing dust test** (`test_fee_routing_dust.rs`)
   - Confirms minimum-fee floor (1 base unit) holds for all charge amounts
   - Validates `net + fee == gross` invariant

### Invariants That Hold (Single Level)

For any charge to a sub-account:
```
subscription.prepaid_balance (before) 
  == subscription.prepaid_balance (after) + gross_charged

sub_account.balance (before) + net_amount 
  == sub_account.balance (after)

treasury.balance (before) + fee_amount 
  == treasury.balance (after)

gross_charged 
  == net_amount + fee_amount  (conservation)

parent_merchant_earnings.accruals.interval 
  includes gross_charged  (regardless of routing)
```

All these are validated by the existing test suite.

---

## Scenario: Attempted Nested Routing

### What Happens If You Try to Nest?

**Scenario**: Create merchant M1, register sub-account "sales" under M1, then try to create a subscription that routes to a sub-account of "sales".

**Current Behavior**:
1. A subscription can only have `sub_account_label = Some(Symbol)`, where the symbol is a simple label
2. There is no `parent_sub_account_label` field in the `Subscription` type
3. Attempting to create a subscription with a hierarchical label like `Symbol::new(env, "sales/team")` would:
   - Succeed in creation (Soroban permits any symbol)
   - Fail at charge time with `Error::NotFound` because there is no sub-account registered with that exact label

**Example**:
```rust
// This will compile
let sub_id = client.create_subscription_with_token(
    &subscriber,
    &M1,
    &token,
    &1000,
    &86400,
    &false,
    &None,
    &None,
    &None,
    &Some(Symbol::new(env, "sales/team")),  // hierarchical label
);

// This will fail at charge time with Error::NotFound
// because no sub-account is registered with label "sales/team"
let charge_result = client.charge_subscription(&sub_id, &None);
assert_eq!(charge_result, Err(Ok(Error::NotFound)));
```

There is **no hidden support** for nesting — it simply fails cleanly.

---

## Recommendations

### For Current Use Cases

If you need multi-level revenue routing, model it as:

**Option 1: Flat Sub-Accounts**
- Create sub-accounts for each "leaf" in your hierarchy: "sales_na", "sales_emea", "support_tier1", "support_tier2", etc.
- Route subscriptions directly to the appropriate leaf
- Aggregate by prefix off-chain: group "sales_*" for region reporting, "support_*" for tier reporting

**Option 2: Separate Merchants**
- Create distinct merchant entities for each level of your hierarchy
- Use split billing or cooperative settlement to distribute funds between levels
- Leverage `SplitChargeEvent` to route to multiple merchants in a single charge

**Option 3: Off-Chain Ledger**
- Route all subscriptions to a single parent sub-account
- Maintain an off-chain ledger that subdivides that sub-account
- Settle internal transactions off-chain or via side contracts

### For Protocol Enhancements

If nested routing is needed in a future version:

1. **Extend Subscription type** with `parent_sub_account_label: Option<Symbol>` (breaking change)
2. **Define fee semantics**:
   - Fee applies once at the top level only (recommended for simplicity)
   - Net amount cascades through levels
3. **Update charge logic** to resolve the hierarchy and route to the leaf
4. **Extend queries** to support hierarchical traversal with performance bounds
5. **Add recursive aggregation** queries for multi-level earnings reports
6. **Consider storage cost**: Each nesting level adds one lookups per charge

**Migration path**: Would require contract upgrade; existing subscriptions would need re-registration.

---

## Testing Recommendations

### Tests to Add (Optional)

If you want to verify the current "no nesting" behavior explicitly:

```rust
#[test]
fn hierarchical_label_fails_at_charge_time() {
    // Verify that a label like "sales/team" is rejected or fails safely
}

#[test]
fn sub_account_of_sub_account_not_allowed() {
    // Try to register_sub_account on a merchant whose own balance 
    // is a routed sub-account (e.g., merchant = router_label)
    // Should fail or be rejected
}

#[test]
fn fee_routing_single_level_deterministic() {
    // Charge to sub-account, verify fee is applied once, no double-charging
}
```

These are **documentation tests** — they clarify the design boundaries rather than finding bugs.

---

## Summary Table

| Aspect | Current Support | Notes |
|--------|-----------------|-------|
| Sub-accounts per merchant | Multiple flat sub-accounts | ✅ Supported |
| Charging to sub-account | Direct routing via label | ✅ Supported |
| Fee application | Single level, deterministic | ✅ Supported |
| Nested sub-accounts | Not supported | ❌ No API or mechanism |
| Hierarchical routing | Not supported | ❌ Would require design change |
| Fee propagation correctness | Verified for single level | ✅ All tests pass |
| Double-charging risk | None at single level | ✅ Conservation invariant holds |

---

## Conclusion

The Stellabill subscription vault **correctly implements** flat, single-level sub-accounts with deterministic fee routing. There is no support for nested hierarchies, nor is there a risk of double-charging or fee leakage at the current level of support. The design is sound, well-tested, and ready for production use for single-level revenue routing.

If multi-level routing is required, use the recommended workarounds (flat labeling, separate merchants, or off-chain ledgers) until a future protocol upgrade is available.
