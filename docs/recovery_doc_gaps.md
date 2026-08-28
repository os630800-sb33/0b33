# Recovery Documentation Gaps Analysis

## Overview

The recovery documentation (`docs/recovery.md`) currently explains the recovery mechanism's purpose, use cases, and governance process, but omits three critical pieces of operational information:

1. **Admin-only authorization model** — Not explicitly stated who can call recovery
2. **Precise definition of "stranded" funds** — How stranded amounts are identified
3. **Query API for identifying stranded amounts** — How to programmatically detect them

---

## Gap 1: Admin Authorization Model (Not Documented)

### Current State

The documentation mentions in the Technical Implementation section:

> Admin Authorization  
> — Only the configured admin address can invoke recovery

But this is vague. It doesn't explain:

- **What authentication mechanism is used?** The code uses `require_admin_auth(env, &admin)?` but the doc doesn't explain what this entails.
- **Does it require signature verification?** Yes — the contract calls `admin.require_auth()` before checking if `admin == stored_admin`.
- **Can multi-sig be used?** The doc suggests it ("Admin key should be a multi-signature wallet") but doesn't clarify that the Soroban SDK's auth model handles this transparently.

### Implementation Details

From `contracts/subscription_vault/src/admin.rs` (lines 528–540):

```rust
pub fn do_recover_stranded_funds(
    env: &Env,
    admin: Address,
    token: Address,
    recipient: Address,
    amount: i128,
    recovery_id: String,
    reason: RecoveryReason,
) -> Result<(), Error> {
    require_admin_auth(env, &admin)?;  // ← Signature check + address match
    
    if amount <= 0 {
        return Err(Error::InvalidRecoveryAmount);
    }
    // ...
}
```

The `require_admin_auth` function:
1. **Requires cryptographic signature** from the `admin` address (Soroban's `Address::require_auth()`).
2. **Validates that the signing address matches the stored admin** in contract storage.

### What Should Be Added

**Authorization model section:**

> ## Authorization Model
>
> The `recover_stranded_funds` function is **admin-only** and requires both:
>
> 1. **Cryptographic signature verification**: The caller must provide a signature from the admin address.  
>    The Soroban SDK validates this signature against the signer's public key on-chain.
>
> 2. **Admin address validation**: The signing address must match the admin address stored in contract state  
>    (set at initialization or via admin rotation).
>
> ### Multi-signature support
>
> Stellar accounts can be configured with **multiple signers and a signing threshold**. If the admin  
> is configured as a multi-sig account, the recovery call automatically requires the threshold number  
> of signatures from authorized signers. This is transparent to the contract; Soroban's auth layer  
> handles threshold validation.
>
> ### Implementation
>
> ```rust
> // Contract code: require_admin_auth(env, &admin)?;
> //   1. env.invoke_contract calls admin.require_auth()
> //   2. Soroban SDK verifies signatures
> //   3. If verified, checks stored admin == caller admin
> //   4. Otherwise returns Error::Unauthorized
> ```

---

## Gap 2: Precise Definition of "Stranded" Funds (Not Documented)

### Current State

The documentation provides four recovery reasons (UserOverpayment, FailedTransfer, ExpiredEscrow, SystemCorrection) but doesn't define what makes funds "stranded" at a technical level.

The intro says:

> Funds that become inaccessible through normal contract operations.

But "inaccessible" is vague. It doesn't explain:

- **How are funds accounted for?** What distinction exists between "accounted" and "stranded"?
- **What makes a fund inaccessible?** Is it unlinked from a subscription? Orphaned in storage?
- **How is the recoverable amount calculated?** What's the formula?

### Implementation Details

From `contracts/subscription_vault/src/admin.rs` (lines 528–562):

```rust
// Validate available recoverable balance
let token_client = token::Client::new(env, &token);
let contract_balance = token_client.balance(&env.current_contract_address());
let accounted_balance = crate::accounting::get_total_accounted(env, &token);

let recoverable = contract_balance
    .checked_sub(accounted_balance)
    .ok_or(Error::Underflow)?;
if amount > recoverable {
    return Err(Error::InsufficientBalance);
}
```

The calculation is:

```
recoverable_amount = contract_token_balance - total_accounted_balance
```

Where:
- **`contract_token_balance`**: The actual USDC (or other token) held by the contract.
- **`total_accounted_balance`**: Sum of all active subscriptions' prepaid balances + all merchant balances.

Funds are "stranded" if they exist in the contract but are not accounted for in any subscription or merchant balance.

From `contracts/subscription_vault/src/queries.rs` (lines 530–570):

```rust
pub fn get_token_reconciliation(env: &Env, token: Address) -> TokenLiabilities {
    let token_client = soroban_sdk::token::Client::new(env, &token);
    let contract_balance = token_client.balance(&env.current_contract_address());

    let total_prepaid = compute_total_prepaid(env, &token);
    let total_merchant_liabilities = compute_total_merchant_liabilities(env, &token, total_prepaid);

    let accounted = total_prepaid
        .checked_add(total_merchant_liabilities)
        .unwrap_or(0i128);
    let recoverable_amount = contract_balance.saturating_sub(accounted).max(0i128);
    // ...
}
```

### What Should Be Added

**Definition of "stranded" section:**

> ## Accounting Model: Defined vs. Stranded Funds
>
> Every token held by the contract must be accounted for in one of two ways:
>
> ### Accounted funds
>
> Funds linked to active contract state and recoverable through normal operations:
>
> 1. **Subscription prepaid balances**: Amount deposited by subscriber, decremented by charges.
> 2. **Merchant accumulated balances**: Amount credited through charges, awaitable via withdrawal.
>
> **Formula for total accounted funds:**
>
> ```
> accounted(token) = sum(subscription.prepaid_balance for all subscriptions)
>                  + sum(merchant_balance for all merchants)
> ```
>
> ### Stranded funds
>
> Funds physically in the contract but not linked to any subscription or merchant balance:
>
> ```
> stranded(token) = contract_token_balance(token) - accounted(token)
> ```
>
> Stranded funds must be **non-negative**. If they're negative, the contract is insolvent  
> (a critical invariant violation).
>
> ### Examples of stranded scenarios
>
> | Scenario | Why Stranded |
> |---|---|
> | User sends USDC directly to contract address | Not attached to any subscription |
> | Upgrade bug leaves tokens in deprecated storage key | Old key no longer indexed in computations |
> | Cancelled subscription; subscriber lost keys | Prepaid balance unreachable, but still in storage |
> | Transfer fails mid-flight; tokens stuck | Funds moved to contract but state update rolled back |
>
> ### Invariant
>
> For every supported token:
>
> ```
> contract_balance(token) >= accounted(token) >= 0
> ```
>
> If this invariant is violated, recovery cannot proceed. Data corruption or an exploit  
> must be investigated.

---

## Gap 3: Query API for Identifying Stranded Amounts (Not Documented)

### Current State

The doc mentions monitoring and auditing but provides only pseudocode:

```javascript
// Pseudocode for monitoring
contract.events.on("recovery", (event) => {
  alert({
    admin: event.admin,
    recipient: event.recipient,
    amount: event.amount,
    reason: event.reason,
    timestamp: event.timestamp,
  });
});
```

It does not explain **how to programmatically determine if funds are stranded before recovery is needed**.

### Implementation Details

The contract exposes a public, unauthenticated query function `get_token_reconciliation` that returns a `TokenLiabilities` struct:

From `contracts/subscription_vault/src/lib.rs` (line 661):

```rust
pub fn get_token_reconciliation(env: &Env, token: Address) -> TokenLiabilities {
    queries::get_token_reconciliation(env, token)
}
```

From `contracts/subscription_vault/src/queries.rs` (lines 532–570):

```rust
pub fn get_token_reconciliation(env: &Env, token: Address) -> TokenLiabilities {
    let token_client = soroban_sdk::token::Client::new(env, &token);
    let contract_balance = token_client.balance(&env.current_contract_address());

    // Compute total prepaid across all subscriptions
    let total_prepaid = compute_total_prepaid(env, &token);

    // Compute total merchant liabilities
    let total_merchant_liabilities = compute_total_merchant_liabilities(env, &token, total_prepaid);

    // Recoverable is the difference between contract balance and accounted funds
    let accounted = total_prepaid
        .checked_add(total_merchant_liabilities)
        .unwrap_or(0i128);
    let recoverable_amount = contract_balance.saturating_sub(accounted).max(0i128);

    let computed_total = accounted.checked_add(recoverable_amount).unwrap_or(0i128);
    let is_balanced = contract_balance == computed_total;

    TokenLiabilities {
        token,
        total_prepaid,
        total_merchant_liabilities,
        recoverable_amount,
        contract_balance,
        computed_total,
        is_balanced,
        normalized_prepaid,
        normalized_merchant_liab,
        normalized_recoverable,
        normalized_contract_balance,
        normalized_computed_total,
    }
}
```

The returned `TokenLiabilities` struct includes:

| Field | Meaning |
|-------|---------|
| `total_prepaid` | Sum of all subscription prepaid balances |
| `total_merchant_liabilities` | Sum of all merchant balances |
| `recoverable_amount` | Stranded amount = contract_balance - (prepaid + merchant_liabilities) |
| `contract_balance` | Actual USDC held by contract |
| `is_balanced` | True if contract_balance == prepaid + merchant + recoverable (no corruption) |

### What Should Be Added

**Query API section:**

> ## Identifying Stranded Funds: Query API
>
> To detect whether stranded funds exist and their amount, use the public, unauthenticated  
> query function:
>
> ```rust
> pub fn get_token_reconciliation(env: &Env, token: Address) -> TokenLiabilities
> ```
>
> ### Return structure
>
> ```rust
> pub struct TokenLiabilities {
>     pub token: Address,
>     pub total_prepaid: i128,              // Sum of all subscription.prepaid_balance
>     pub total_merchant_liabilities: i128, // Sum of all merchant earnings
>     pub recoverable_amount: i128,         // Stranded funds (token balance - liabilities)
>     pub contract_balance: i128,           // Actual token balance in contract
>     pub computed_total: i128,             // total_prepaid + total_merchant + recoverable
>     pub is_balanced: bool,                // True if contract_balance == computed_total
>     pub normalized_prepaid: u64,          // total_prepaid normalized to token decimals
>     pub normalized_merchant_liab: u64,    // total_merchant normalized to token decimals
>     pub normalized_recoverable: u64,      // recoverable normalized to token decimals
>     pub normalized_contract_balance: u64, // contract_balance normalized to token decimals
>     pub normalized_computed_total: u64,   // computed_total normalized to token decimals
> }
> ```
>
> ### Interpreting the result
>
> #### Step 1: Check for data corruption
>
> ```rust
> let liabilities = client.get_token_reconciliation(&usdc_token);
> if !liabilities.is_balanced {
>     eprintln!("ALERT: Contract is out of balance!");
>     eprintln!("  Expected: {}", liabilities.computed_total);
>     eprintln!("  Actual: {}", liabilities.contract_balance);
>     // → Stop. Investigate. Do not attempt recovery.
> }
> ```
>
> #### Step 2: Identify stranded amount
>
> ```rust
> if liabilities.recoverable_amount > 0 {
>     println!("Stranded funds detected!");
>     println!("  Amount (base units): {}", liabilities.recoverable_amount);
>     println!("  Amount (normalized): {}", liabilities.normalized_recoverable);
> }
> ```
>
> #### Step 3: Validate recovery won't harm active subscriptions or merchants
>
> ```rust
> // Safe recovery amount = all stranded funds (they are not linked to anyone)
> let max_recoverable = liabilities.recoverable_amount;
>
> // Verify that prepaid + merchant balances are stable
> assert!(liabilities.total_prepaid >= 0);
> assert!(liabilities.total_merchant_liabilities >= 0);
> ```
>
> ### Example: CLI usage
>
> ```bash
> # Query via soroban CLI
> soroban contract invoke \
>   --id <CONTRACT_ID> \
>   --rpc-url https://soroban-testnet.stellar.org \
>   -- \
>   get_token_reconciliation \
>   --token <USDC_TOKEN_ADDRESS>
>
> # Response (formatted):
> {
>   "token": "...",
>   "total_prepaid": 1000000000,        // 1000 USDC (6 decimals)
>   "total_merchant_liabilities": 500000000, // 500 USDC
>   "recoverable_amount": 50000000,    // 50 USDC STRANDED
>   "contract_balance": 1550000000,    // 1550 USDC
>   "computed_total": 1550000000,
>   "is_balanced": true,
>   "normalized_prepaid": 1000,
>   "normalized_merchant_liab": 500,
>   "normalized_recoverable": 50,      // ← Human-readable stranded amount
>   "normalized_contract_balance": 1550,
>   "normalized_computed_total": 1550
> }
> ```
>
> ### Programmatic workflow (pseudo-code)
>
> ```python
> import json
> from soroban_sdk import Client
>
> def check_for_stranded_funds(contract_id: str, token_id: str) -> dict:
>     \"\"\"Returns info about stranded funds for a token.\"\"\"
>     client = Client(...)
>     
>     liabilities = client.get_token_reconciliation(contract_id, token_id)
>     
>     # Fail fast on imbalance
>     if not liabilities['is_balanced']:
>         return {
>             'status': 'ERROR_CORRUPTION',
>             'message': 'Contract is out of balance. Do not recover.',
>             'details': liabilities
>         }
>     
>     # Report stranded amount
>     return {
>         'status': 'OK',
>         'stranded_amount_base_units': liabilities['recoverable_amount'],
>         'stranded_amount_normalized': liabilities['normalized_recoverable'],
>         'total_prepaid': liabilities['normalized_prepaid'],
>         'total_merchant_liabilities': liabilities['normalized_merchant_liab'],
>     }
> ```
>
> ### Security notes
>
> - **No authentication required**: `get_token_reconciliation` is deliberately public.  
>   Anyone can query stranded amounts; transparency is intentional.
> - **TTL-safe**: The function computes totals on-the-fly from ledger state;  
>   no caching means it reflects the current state.
> - **Deterministic**: Calling twice in the same ledger produces the same result.

---

## Summary of Missing Content

| Gap | Location in Doc | Why It Matters | Recommended Addition |
|-----|-----------------|---|---|
| **Admin-only auth model** | "Admin Authorization" subsection of "Technical Implementation" | Developers/auditors need to verify that recovery requires multi-sig or key control | Explain `require_auth()`, address validation, multi-sig support, Soroban auth model |
| **Definition of "stranded"** | "Purpose" and "Recovery Scenarios" sections | Operators need a precise, auditable definition of what funds qualify as recoverable | Define `stranded = contract_balance - (prepaid + merchant)`, show invariant, list example scenarios |
| **Query API** | "Monitoring and Auditing" section | Operators need a concrete method to programmatically detect stranded funds before recovery | Document `get_token_reconciliation`, explain struct fields, provide CLI and Python examples, show validation workflow |

---

## Recommendations

1. **Add "Authorization Model" subsection** to "Technical Implementation" explaining Soroban's auth flow.

2. **Add "Accounting Model: Defined vs. Stranded Funds" section** with the formula and invariant.

3. **Replace the pseudocode in "Monitoring and Auditing"** with actual `get_token_reconciliation` documentation, struct definition, and end-to-end workflow examples.

4. **Add a "Querying for Stranded Amounts" subsection** to "Before Recovery" in the governance section, linking to the query API and validation steps.

5. **Cross-reference docs**: Link to `docs/storage_layout.md` or `docs/reconciliation_strategy.md` for deeper technical details on accounting.
