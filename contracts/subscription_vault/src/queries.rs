//! Read-only entrypoints and helpers: get_subscription, estimate_topup, list_subscriptions_by_subscriber.
//!
//! **PRs that only add or change read-only/query behavior should edit this file only.**
//!
//! ## Security classification: emergency_stop view surface (#847)
//!
//! All functions in this module are **deliberately unauthenticated and unguarded**
//! by the emergency-stop circuit breaker. This is intentional: read-only views
//! carry no financial risk and must remain available during an incident so that
//! auditors, subscribers, and merchants can inspect state without interruption.
//!
//! The table below documents the safety classification of every view function:
//!
//! | Function | Returns | Emergency-stop safe? | Bypass risk |
//! |---|---|---|---|
//! | `get_subscription` | Subscription record (subscriber, merchant, balance, status, …) | ✅ Yes | None — no admin secret or timelock exposed |
//! | `estimate_topup_for_intervals` | Required top-up amount (pure math on subscription.amount) | ✅ Yes | None |
//! | `get_subscriptions_by_merchant` | Slice of subscription records | ✅ Yes | None |
//! | `get_subscriptions_by_merchant_paginated` | Paginated subscription records with cursor | ✅ Yes | None |
//! | `get_merchant_subscription_count` | Index length (u32) | ✅ Yes | None |
//! | `get_token_subscription_count` | Index length (u32) | ✅ Yes | None |
//! | `get_subscriptions_by_token` | Slice of subscription records | ✅ Yes | None |
//! | `list_subscriptions_by_token` | Paginated subscription records with cursor | ✅ Yes | None |
//! | `get_next_charge_info` | Projected next-charge timestamp and status | ✅ Yes | None |
//! | `compute_next_charge_info` | Pure computation on a Subscription value | ✅ Yes | None |
//! | `get_cap_info` | Lifetime cap / charged totals | ✅ Yes | None |
//! | `get_plan_max_active_subs` | Per-plan active-subscription limit | ✅ Yes | None |
//! | `get_merchant_max_subs` | Per-merchant active-subscription limit | ✅ Yes | None |
//! | `get_subscriptions_by_subscriber` | Slice of subscription records | ✅ Yes | None |
//! | `list_subscriptions_by_subscriber` | Paginated subscription IDs | ✅ Yes | None |
//! | `list_recoverable_subscriptions` | Paginated stranded (cancelled + non-zero prepaid) subscriptions | ✅ Yes | None |
//! | `list_credit_limits` | Active credit-limit grants for a merchant | ✅ Yes | None |
//! | `list_credit_limits_paginated` | Cursor-paginated active credit-limit grants | ✅ Yes | None |
//! | `get_token_reconciliation` | Accounting totals for a token (no secrets) | ✅ Yes | None — balance totals are public; no admin credential exposed |
//! | `get_contract_reconciliation_summary` | Multi-token accounting summaries | ✅ Yes | None |
//! | `generate_reconciliation_proof` | Auditable accounting snapshot | ✅ Yes | None |
//! | `get_token_reconciliation_paginated` | Partial (bounded) token accounting subtotals | ✅ Yes | None |
//! | `generate_reconciliation_proof_paginated` | Partial (bounded) auditable accounting snapshots | ✅ Yes | None |
//! | `get_contract_reconciliation_summary_paginated` | Partial multi-token accounting subtotals | ✅ Yes | None |
//! | `query_prepaid_balances_paginated` | Partial prepaid totals (paginated) | ✅ Yes | None |
//!
//! ### Why no view function can bypass the emergency stop
//!
//! The emergency-stop flag (`DataKey::EmergencyStop`) is checked via
//! `require_not_emergency_stop` on **every mutating** entry-point before any
//! state change occurs. Read-only functions never call `write_config`, never
//! transfer tokens, and never advance any nonce or counter. An attacker who
//! calls any view during an active emergency stop gains only data that is already
//! publicly visible on the ledger; they cannot trigger a blocked mutation or
//! extract a signing key.
//!
//! `get_admin()` (in `lib.rs`) returns the admin address by design. Knowing the
//! admin address does **not** allow bypassing the stop: the stop check runs before
//! any admin-gated write, and the emergency-stop doc explicitly lists `get_admin`
//! as safe during an active stop.
//!
//! ### Pre-init behaviour
//!
//! Before `init` is called, views that depend on a stored subscription
//! (`get_subscription`, `estimate_topup_for_intervals`, `get_next_charge_info`,
//! `get_cap_info`) return `Error::NotFound` because no subscription exists at
//! ID 0.  Count / index views (`get_merchant_subscription_count`,
//! `get_token_subscription_count`, `get_plan_max_active_subs`,
//! `get_merchant_max_subs`) return `0` or `u32::MAX` (the "no limit" sentinel).
//! Reconciliation views (`get_token_reconciliation`, `generate_reconciliation_proof`)
//! require a valid token address with a deployed token contract; calling them
//! before init with an arbitrary address will trap on the cross-contract call.
//! `list_subscriptions_by_subscriber` returns empty results safely because a
//! missing `SubscriberSubs` index defaults to an empty list, and
//! `query_prepaid_balances_paginated` returns empty results safely because
//! `DataKey::NextId` defaults to `0`. Likewise `list_recoverable_subscriptions`
//! returns an empty page safely before `init` for the same reason.
//!
//! ## Pagination invariants (off-chain / indexers)
//!
//! - **`list_subscriptions_by_subscriber`**: Backed by the `SubscriberSubs` secondary index
//!   (insertion order, ascending id). `start_from_id` is inclusive. Continue with
//!   `next_start_id` when present (next id to scan). Each call inspects at most
//!   `MAX_SCAN_DEPTH` index entries — if the scan budget is exhausted before the page is
//!   full, `next_start_id` is set to the resume point so callers can chain pages.
//! - **`list_recoverable_subscriptions`**: Full-ID scan (cancelled subscriptions are removed
//!   from every secondary index). `cursor` is an inclusive lower bound, `None` meaning ID `0`.
//!   Results are ascending by id. Continue with `next_cursor` when present; when it is `None`
//!   the scan reached the end of the ID space and enumeration is complete, so an empty page
//!   with `None` means nothing is stranded. Each call inspects at most `MAX_SCAN_DEPTH` ids.
//! - **`get_subscriptions_by_merchant`**: Results follow the order of ids in the on-chain index
//!   (`MerchantSubs`), which is insertion order (ascending id order for subscriptions created
//!   through this contract). `start` is a 0-based offset into that id list. Passing
//!   `reverse = true` walks the same index from its end, returning newest-first
//!   (reverse-chronological) results without inverting the index. Missing subscription records
//!   are skipped; callers should use [`get_merchant_subscription_count`] for the index length,
//!   not `result.len()`, when paginating.
//! - **`get_subscriptions_by_merchant_paginated`**: Cursor-based variant of the above. The
//!   cursor is an offset into whichever ordering `reverse` selects, so cursors are
//!   direction-specific and must be replayed with the same `reverse` value.
//! - **`get_subscriptions_by_token`**: Results follow the order of ids in the on-chain index
//!   (`token_subs`), which is insertion order. `start` is a 0-based offset into that id list.
//! - **`list_subscriptions_by_token`**: Cursor-based traversal of the same token index in
//!   insertion order. `cursor` is a 0-based offset (`None` starts at the beginning); continue
//!   with `next_cursor`. The returned `total` is the token index length.
//! - **`list_credit_limits_paginated`**: Cursor is a 0-based offset into the
//!   `MerchantCreditLimits` index (`None` starts at the beginning). Each call consumes at most
//!   `limit` index positions; revoked/expired grants inside the window are skipped, so a page can
//!   hold fewer than `limit` entries while `next_cursor` is still `Some`. Continue with
//!   `next_cursor` until it is `None`. `total` is the index length.
//! - **`get_token_reconciliation_paginated`** / **`generate_reconciliation_proof_paginated`**:
//!   Cursor is a subscription-ID resume point (`None` starts at ID `0`). Each call scans at most
//!   `MAX_PREPAID_SCAN_DEPTH` IDs and returns the prepaid subtotal for that window; sum
//!   `partial_prepaid` across pages until `has_more` is `false`. These are the bounded
//!   counterparts of the full-scan `get_token_reconciliation` / `generate_reconciliation_proof`.
//! - **`get_contract_reconciliation_summary_paginated`**: Two-level iteration. `subscription_cursor`
//!   is a subscription-ID resume point for the **current** token window; iterate it until the
//!   returned `next_subscription_cursor` is `None`, accumulating each token's `partial_prepaid`,
//!   then advance to `next_token_index` (if `Some`) and restart with `subscription_cursor = None`.
//!   The per-call read budget (`MAX_PREPAID_SCAN_DEPTH`) is shared across the tokens in the window.
//!
//! ## Read complexity per endpoint
//!
//! | Endpoint | Storage reads | Notes |
//! |---|---|---|
//! | `get_subscription` | 1 | Direct key lookup |
//! | `estimate_topup_for_intervals` | 1 | Calls `get_subscription` |
//! | `get_subscriptions_by_merchant` | 1 + limit | 1 index read + up to `limit` sub reads |
//! | `get_subscriptions_by_merchant_paginated` | 1 + limit | 1 index read + up to `limit` sub reads |
//! | `get_merchant_subscription_count` | 1 | Index length only |
//! | `get_subscriptions_by_token` | 1 + limit | 1 index read + up to `limit` sub reads |
//! | `list_subscriptions_by_token` | 1 + limit | 1 index read + up to `limit` sub reads |
//! | `get_token_subscription_count` | 1 | Index length only |
//! | `compute_next_charge_info` | 0 | Pure computation |
//! | `get_cap_info` | 1 | Calls `get_subscription` |
//! | `get_plan_max_active_subs` | 1 | Direct key lookup |
//! | `list_subscriptions_by_subscriber` | 1 + up to MAX_SCAN_DEPTH | 1 index read + capped scan of that subscriber's ids |
//! | `list_recoverable_subscriptions` | up to MAX_SCAN_DEPTH | Capped full-ID scan; cancelled subs are absent from every secondary index |
//! | `list_credit_limits` | 1 + N | 1 index read + one record read per index entry (unbounded) |
//! | `list_credit_limits_paginated` | 1 + limit | 1 index read + up to `limit` record reads |
//! | `get_token_reconciliation` | up to MAX_SCAN_DEPTH | Full `0..NextId` subscription scan (unbounded on large contracts) |
//! | `get_token_reconciliation_paginated` | up to MAX_PREPAID_SCAN_DEPTH | Bounded ID window + 1 token balance read |
//! | `generate_reconciliation_proof` | up to MAX_SCAN_DEPTH | Full `0..NextId` subscription scan (unbounded on large contracts) |
//! | `generate_reconciliation_proof_paginated` | up to MAX_PREPAID_SCAN_DEPTH | Bounded ID window + 1 token balance read |
//! | `get_contract_reconciliation_summary` | tokens × MAX_SCAN_DEPTH | Per-token full subscription scan (unbounded on large contracts) |
//! | `get_contract_reconciliation_summary_paginated` | up to MAX_PREPAID_SCAN_DEPTH | Shared across the token window |
//! | `query_prepaid_balances_paginated` | up to MAX_PREPAID_SCAN_DEPTH | Bounded ID window |
//!
//! **Index deserialization note**: merchant and token subscription queries read the
//! entire index `Vec<u32>` from a single storage key before slicing it. For merchants
//! or tokens with very large index lists the deserialization cost grows with the list
//! length, even when only `limit` entries are needed. Use
//! [`get_merchant_subscription_count`] / [`get_token_subscription_count`] to estimate
//! index size before paginating.
//!
//! ## Key registry: DataKey variants used here
//! - `DataKey::Sub(u32)`
//! - `DataKey::MerchantSubs(Address)`
//! - `DataKey::TokenSubs(Address)`
//! - `DataKey::PlanMaxActive(u32)`
//! - `DataKey::MerchantCreditLimits(Address)`
//! - `DataKey::CreditLimit(Address, Address)`
//! - `DataKey::AcceptedTokens`
//! - `DataKey::NextId`

use crate::safe_math::{safe_mul, safe_sub};
use crate::subscription::{read_subscription, try_get_subscription};
use crate::types::{
    CapInfo, DataKey, Error, NextChargeInfo, Subscription, SubscriptionStatus,
    SubscriptionsMerchantPage, SubscriptionsTokenPage,
};
use soroban_sdk::{contracttype, Address, Env, Vec};

/// Maximum `limit` for [`get_subscriptions_by_merchant`] and [`get_subscriptions_by_token`]
/// (aligned with [`list_subscriptions_by_subscriber`]).
pub const MAX_SUBSCRIPTION_LIST_PAGE: u32 = 100;

/// Maximum offset that [`get_subscriptions_by_merchant_paginated`] will serve.
///
/// This bounds total enumeration across chained pages, not just the work in a
/// single call. Once the cursor reaches this depth, the query returns an empty
/// terminal page.
pub const MAX_MERCHANT_CURSOR_DEPTH: u32 = 1_000;

/// Hard upper bound on the `limit` parameter for [`list_subscriptions_by_subscriber`].
///
/// Requests with `limit > MAX_QUERY_RESULTS` are rejected with
/// [`Error::InvalidExportLimit`] (code 6004) before any storage reads occur.
/// This cap prevents a single call from enumerating an unbounded number of
/// subscription IDs, bounding per-call Soroban resource consumption.
pub const MAX_QUERY_RESULTS: u32 = 100;

/// Maximum number of subscription IDs scanned in a single
/// [`list_subscriptions_by_subscriber`] call.
///
/// Because `list_subscriptions_by_subscriber` performs a sequential ID scan (rather
/// than using a secondary index), an unbounded scan over a large contract is
/// proportionally expensive in storage reads. This constant caps the scan window
/// per call so that any single invocation reads at most `MAX_SCAN_DEPTH` IDs.
///
/// When the budget is exhausted before the requested page is full, the returned
/// `next_start_id` points to the first unscanned ID so callers can issue a
/// follow-up call to resume. Subscribers whose subscriptions are spread sparsely
/// over a large ID range may therefore need more round-trips to collect all IDs.
///
/// **Rationale**: 1 000 reads per call is the practical upper bound for a
/// sensible contract interaction; it keeps per-call cost predictable while still
/// allowing full enumeration via chaining.
pub const MAX_SCAN_DEPTH: u32 = 1_000;

/// Hard upper bound on the `limit` parameter for
/// [`list_credit_limits_paginated`].
///
/// The `MerchantCreditLimits` index can grow without bound as a merchant issues
/// grants, so an uncapped call would read every entry in the index. This cap
/// bounds the number of index positions consumed (and therefore the number of
/// `CreditLimit` record reads) per call. Requests with `limit == 0` or
/// `limit > MAX_CREDIT_LIMITS_PAGE` are rejected with [`Error::InvalidInput`].
pub const MAX_CREDIT_LIMITS_PAGE: u32 = 100;

pub fn get_subscription(env: &Env, subscription_id: u32) -> Result<Subscription, Error> {
    let mut sub = try_get_subscription(env, subscription_id)?;

    // `GracePeriod` is persisted, but the transition out of it only happens on
    // the next charge attempt. If the grace window has already elapsed with no
    // charge attempted yet, the stored status is stale — report the effective
    // status (`InsufficientBalance`) instead of a `GracePeriod` that has
    // functionally already expired.
    if sub.status == SubscriptionStatus::GracePeriod {
        if let Some(grace_start) = sub.grace_start_timestamp {
            let grace_duration = crate::admin::get_grace_period(env).unwrap_or(0);
            let grace_expires = grace_start.saturating_add(grace_duration);
            if grace_duration == 0 || env.ledger().timestamp() >= grace_expires {
                sub.status = SubscriptionStatus::InsufficientBalance;
            }
        }
    }

    Ok(sub)
}

pub fn estimate_topup_for_intervals(
    env: &Env,
    subscription_id: u32,
    num_intervals: u32,
) -> Result<i128, Error> {
    let sub = get_subscription(env, subscription_id)?;

    if num_intervals == 0 {
        return Ok(0);
    }

    let intervals_i128: i128 = num_intervals.into();
    let required = safe_mul(sub.amount, intervals_i128)?;

    let topup = if required <= sub.prepaid_balance {
        0
    } else {
        safe_sub(required, sub.prepaid_balance)?
    };
    Ok(topup)
}

/// Reads up to `limit` subscription records starting at logical offset `start`
/// into a merchant's id index.
///
/// When `reverse` is `true`, the index is traversed from its end so the newest
/// subscriptions (highest ids) come first. The index is never copied or
/// reordered: each logical position is mapped to a physical index with
/// `len - 1 - position`, so the storage cost stays `1 + limit` reads regardless
/// of the ordering chosen.
fn read_merchant_page(
    env: &Env,
    ids: &Vec<u32>,
    start: u32,
    limit: u32,
    reverse: bool,
) -> Vec<Subscription> {
    let len = ids.len();
    let end = start.saturating_add(limit).min(len);
    let mut out = Vec::new(env);
    let mut position = start;
    while position < end {
        let index = if reverse {
            len - 1 - position
        } else {
            position
        };
        if let Some(sub_id) = ids.get(index) {
            if let Some(sub) = read_subscription(env, sub_id) {
                out.push_back(sub);
            }
        }
        position += 1;
    }
    out
}

/// Returns subscriptions for a merchant, paginated by offset into the merchant id index.
///
/// `limit` must be in `1..=MAX_SUBSCRIPTION_LIST_PAGE`.
///
/// `reverse = false` returns results in stable index order (insertion order,
/// oldest first). `reverse = true` returns them in reverse-chronological order
/// (newest first) without materialising or inverting the whole index. `start`
/// always counts from the beginning of the selected ordering.
pub fn get_subscriptions_by_merchant(
    env: &Env,
    merchant: Address,
    start: u32,
    limit: u32,
    reverse: bool,
) -> Result<Vec<Subscription>, Error> {
    if limit == 0 || limit > MAX_SUBSCRIPTION_LIST_PAGE {
        return Err(Error::InvalidInput);
    }
    let ids = crate::merchant::get_merchant_subscription_ids(env, &merchant);

    if start >= ids.len() {
        return Ok(Vec::new(env));
    }

    Ok(read_merchant_page(env, &ids, start, limit, reverse))
}

/// Returns the number of subscriptions for a given merchant.
pub fn get_merchant_subscription_count(env: &Env, merchant: Address) -> u32 {
    let ids = crate::merchant::get_merchant_subscription_ids(env, &merchant);
    ids.len()
}

/// Returns subscriptions for a merchant with cursor-based pagination.
///
/// # Arguments
/// - `merchant`: The merchant address whose subscriptions to fetch
/// - `cursor`: Optional starting position (0-based offset into the merchant's
///   subscription list *as ordered by `reverse`*). Pass `None` to start from the
///   beginning.
/// - `limit`: Number of subscriptions to return (must be in `1..=MAX_SUBSCRIPTION_LIST_PAGE`)
/// - `reverse`: `false` for oldest-first (insertion order), `true` for
///   newest-first (reverse-chronological)
///
/// # Returns
/// A `SubscriptionsMerchantPage` containing:
/// - `subscriptions`: Vector of subscription records for this page
/// - `next_cursor`: Offset to use for the next page (if more results exist)
/// - `total`: Total number of subscriptions for this merchant
///
/// Cursor traversal is capped at [`MAX_MERCHANT_CURSOR_DEPTH`] index entries.
/// The page is truncated at that boundary, and requests at or beyond it return
/// an empty terminal page (a tombstone) with `next_cursor == None`.
///
/// # Errors
/// - `InvalidInput` if limit is 0 or exceeds `MAX_SUBSCRIPTION_LIST_PAGE`
///
/// # Ordering
/// With `reverse = false`, results follow insertion order (ascending
/// subscription ID order). With `reverse = true`, the index is walked from its
/// end so the newest subscriptions come first; the index itself is never
/// inverted. Missing subscription records are skipped; use `total` to determine
/// the actual count.
///
/// Cursors are **direction-specific**: `next_cursor` is an offset into the
/// ordering that produced it, so replay it with the same `reverse` value.
/// Mixing orderings with a cursor from the other direction yields an undefined
/// page.
pub fn get_subscriptions_by_merchant_paginated(
    env: &Env,
    merchant: Address,
    cursor: Option<u32>,
    limit: u32,
    reverse: bool,
) -> Result<SubscriptionsMerchantPage, Error> {
    if limit == 0 || limit > MAX_SUBSCRIPTION_LIST_PAGE {
        return Err(Error::InvalidInput);
    }

    let ids = crate::merchant::get_merchant_subscription_ids(env, &merchant);
    let total = ids.len();

    let start = cursor.unwrap_or(0);
    if start >= total || start >= MAX_MERCHANT_CURSOR_DEPTH {
        return Ok(SubscriptionsMerchantPage {
            subscriptions: Vec::new(env),
            next_cursor: None,
            total,
        });
    }

    let end = start
        .saturating_add(limit)
        .min(total)
        .min(MAX_MERCHANT_CURSOR_DEPTH);
    let subscriptions = read_merchant_page(env, &ids, start, end - start, reverse);

    let next_cursor = if end < total && end < MAX_MERCHANT_CURSOR_DEPTH {
        Some(end)
    } else {
        None
    };

    Ok(SubscriptionsMerchantPage {
        subscriptions,
        next_cursor,
        total,
    })
}

/// Number of subscription ids indexed for this token (length of the `token_subs` list).
pub fn get_token_subscription_count(env: &Env, token: Address) -> u32 {
    let key = DataKey::TokenSubs(token);
    let ids: Vec<u32> = env.storage().instance().get(&key).unwrap_or(Vec::new(env));
    ids.len()
}

/// Returns subscriptions for a settlement token, paginated by offset into the token id index.
///
/// `limit` must be in `1..=MAX_SUBSCRIPTION_LIST_PAGE`. Ordering is stable index order (insertion order).
pub fn get_subscriptions_by_token(
    env: &Env,
    token: Address,
    start: u32,
    limit: u32,
) -> Result<Vec<Subscription>, Error> {
    if limit == 0 || limit > MAX_SUBSCRIPTION_LIST_PAGE {
        return Err(Error::InvalidInput);
    }
    let key = DataKey::TokenSubs(token);
    let ids: Vec<u32> = env.storage().instance().get(&key).unwrap_or(Vec::new(env));
    let len = ids.len();
    if start >= len {
        return Ok(Vec::new(env));
    }
    let end = if start + limit > len {
        len
    } else {
        start + limit
    };
    let mut out = Vec::new(env);
    for id in ids.iter().skip(start as usize).take((end - start) as usize) {
        if let Some(sub) = read_subscription(env, id) {
            out.push_back(sub);
        }
    }
    Ok(out)
}

/// Returns subscriptions for a settlement token with cursor-based pagination.
///
/// Results follow insertion order in the token index. `cursor` is a 0-based
/// offset; pass the returned `next_cursor` to fetch the next page. `total`
/// reports the current number of IDs in the token index.
///
/// `limit` must be in `1..=MAX_SUBSCRIPTION_LIST_PAGE`.
pub fn list_subscriptions_by_token(
    env: &Env,
    token: Address,
    cursor: Option<u32>,
    limit: u32,
) -> Result<SubscriptionsTokenPage, Error> {
    if limit == 0 || limit > MAX_SUBSCRIPTION_LIST_PAGE {
        return Err(Error::InvalidInput);
    }

    let key = DataKey::TokenSubs(token);
    let ids: Vec<u32> = env.storage().instance().get(&key).unwrap_or(Vec::new(env));
    let total = ids.len();
    let start = cursor.unwrap_or(0);
    if start >= total {
        return Ok(SubscriptionsTokenPage {
            subscriptions: Vec::new(env),
            next_cursor: None,
            total,
        });
    }

    let end = start.saturating_add(limit).min(total);
    let mut subscriptions = Vec::new(env);
    for id in ids.iter().skip(start as usize).take((end - start) as usize) {
        if let Some(subscription) = read_subscription(env, id) {
            subscriptions.push_back(subscription);
        }
    }

    Ok(SubscriptionsTokenPage {
        subscriptions,
        next_cursor: if end < total { Some(end) } else { None },
        total,
    })
}

/// Returns full next charge information for a subscription.
pub fn get_next_charge_info(env: &Env, subscription_id: u32) -> Result<NextChargeInfo, Error> {
    let sub = get_subscription(env, subscription_id)?;
    Ok(compute_next_charge_info(env, &sub))
}

/// Computes the estimated next charge timestamp and status for a subscription.
pub fn compute_next_charge_info(env: &Env, subscription: &Subscription) -> NextChargeInfo {
    let next_charge_timestamp = subscription
        .last_payment_timestamp
        .saturating_add(subscription.interval_seconds);

    let is_charge_expected = match subscription.status {
        SubscriptionStatus::Active => true,
        SubscriptionStatus::GracePeriod => true,
        SubscriptionStatus::InsufficientBalance => false,
        SubscriptionStatus::Paused => false,
        SubscriptionStatus::Cancelled => false,
        SubscriptionStatus::Expired => false,
        SubscriptionStatus::Archived => false,
    };

    let reason = match subscription.status {
        SubscriptionStatus::Active => soroban_sdk::symbol_short!("active"),
        SubscriptionStatus::GracePeriod => soroban_sdk::symbol_short!("grace"),
        SubscriptionStatus::InsufficientBalance => soroban_sdk::symbol_short!("insuf_bal"),
        SubscriptionStatus::Paused => soroban_sdk::symbol_short!("paused"),
        SubscriptionStatus::Cancelled => soroban_sdk::symbol_short!("cancelled"),
        SubscriptionStatus::Expired => soroban_sdk::symbol_short!("expired"),
        SubscriptionStatus::Archived => soroban_sdk::symbol_short!("archived"),
    };

    let grace_deadline = subscription
        .grace_start_timestamp
        .map(|start| start.saturating_add(crate::admin::get_grace_period(env).unwrap_or(0)));

    NextChargeInfo {
        next_charge_timestamp,
        is_charge_expected,
        status: subscription.status,
        reason,
        amount: subscription.amount,
        token: subscription.token.clone(),
        grace_deadline,
    }
}

/// Returns lifetime cap information for a subscription.
pub fn get_cap_info(env: &Env, subscription_id: u32) -> Result<CapInfo, Error> {
    let sub = get_subscription(env, subscription_id)?;

    let (remaining_cap, cap_reached) = match sub.lifetime_cap {
        Some(cap) => {
            let remaining = crate::subscription::lifetime_cap_remaining(cap, sub.lifetime_charged);
            (
                Some(remaining),
                crate::subscription::lifetime_cap_reached(cap, sub.lifetime_charged),
            )
        }
        None => (None, false),
    };

    Ok(CapInfo {
        lifetime_cap: sub.lifetime_cap,
        lifetime_charged: sub.lifetime_charged,
        remaining_cap,
        cap_reached,
    })
}

/// Returns the configured max-active-subscriptions limit for a plan template.
///
/// A return value of `0` means no limit is enforced for that plan.
/// The plan must exist; returns `0` if no limit has been explicitly set.
pub fn get_plan_max_active_subs(env: &Env, plan_template_id: u32) -> u32 {
    let key = DataKey::PlanMaxActive(plan_template_id);
    env.storage().instance().get(&key).unwrap_or(0)
}

/// Returns the configured max-active-subscriptions limit for a merchant.
///
/// A return value of `u32::MAX` means no limit is enforced for that merchant.
pub fn get_merchant_max_subs(env: &Env, merchant: Address) -> u32 {
    let key = DataKey::MerchantMaxSubs(merchant);
    env.storage().instance().get(&key).unwrap_or(u32::MAX)
}

/// Result of a paginated query for subscriptions by subscriber.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubscriptionsPage {
    pub subscription_ids: Vec<u32>,
    pub next_start_id: Option<u32>,
}

/// Returns subscription records for a given subscriber, paginated by offset into
/// the subscriber's subscription index.
///
/// `offset` is a 0-based index into the subscriber's `SubscriberSubs` list.
/// Returns at most `limit` records in stable insertion order (ascending subscription
/// ID). If `offset` is at or past the end of the list, an empty `Vec` is returned.
///
/// `limit` must be in `1..=MAX_SUBSCRIPTION_LIST_PAGE`; returns
/// [`Error::InvalidInput`] otherwise.
///
/// Missing subscription records (removed from persistent storage but still present
/// in the index) are silently skipped, so the returned `Vec` may be shorter than
/// `limit` even before the end of the index.
pub fn get_subscriptions_by_subscriber(
    env: &Env,
    subscriber: Address,
    offset: u32,
    limit: u32,
) -> Result<Vec<Subscription>, Error> {
    if limit == 0 || limit > MAX_SUBSCRIPTION_LIST_PAGE {
        return Err(Error::InvalidInput);
    }
    let ids: Vec<u32> = env
        .storage()
        .instance()
        .get(&DataKey::SubscriberSubs(subscriber))
        .unwrap_or(Vec::new(env));
    let len = ids.len();
    if offset >= len {
        return Ok(Vec::new(env));
    }
    let end = (offset + limit).min(len);
    let mut out = Vec::new(env);
    for id in ids.iter().skip(offset as usize).take((end - offset) as usize) {
        if let Some(sub) = read_subscription(env, id) {
            out.push_back(sub);
        }
    }
    Ok(out)
}

/// Get all subscription IDs for a given subscriber with pagination support.
///
/// ## Complexity
///
/// O(min(`MAX_SCAN_DEPTH`, entries in `DataKey::SubscriberSubs(subscriber)`))
/// storage reads per call — backed by the per-subscriber secondary index
/// maintained at subscription creation (and trimmed on cancellation/transfer),
/// instead of scanning every subscription ID ever issued. At most
/// [`MAX_SCAN_DEPTH`] index entries are inspected per call; if the budget is
/// exhausted before `limit` matching IDs are found, `next_start_id` is set to
/// the first unscanned ID so the caller can resume with another call.
///
/// ## Pagination
///
/// - `start_from_id`: inclusive lower bound on IDs to scan (use `0` for first page).
/// - `limit`: how many matching IDs to return; must be in `1..=MAX_SUBSCRIPTION_LIST_PAGE`.
/// - `next_start_id`: when `Some(id)`, pass it as `start_from_id` to fetch the next
///   page.  `None` means there are no more IDs to scan in the current budget window.
///
/// ## Security note
///
/// The scan cap prevents a single transaction from performing an unbounded number
/// of storage reads under adversarial conditions (e.g. a subscriber with millions
/// of historical subscriptions). The cap does **not** affect correctness — it only
/// splits work across more calls.
pub fn list_subscriptions_by_subscriber(
    env: &Env,
    subscriber: Address,
    start_from_id: u32,
    limit: u32,
) -> Result<SubscriptionsPage, Error> {
    if limit == 0 || limit > MAX_QUERY_RESULTS {
        return Err(Error::InvalidExportLimit);
    }

    let next_id: u32 = crate::admin::read_config(env, &DataKey::NextId).unwrap_or(0);
    if (next_id == 0 && start_from_id > 0) || (next_id > 0 && start_from_id >= next_id) {
        return Err(Error::InvalidInput);
    }

    let index: Vec<u32> = env
        .storage()
        .instance()
        .get(&DataKey::SubscriberSubs(subscriber))
        .unwrap_or(Vec::new(env));

    let mut subscription_ids = Vec::new(env);
    let mut next_start_id: Option<u32> = None;
    let mut scanned: u32 = 0;

    for id in index.iter() {
        if id < start_from_id {
            continue;
        }
        if scanned >= MAX_SCAN_DEPTH {
            next_start_id = Some(id);
            break;
        }
        scanned += 1;

        if subscription_ids.len() < limit {
            subscription_ids.push_back(id);
        } else {
            // Page is full; resume from this ID on the next call.
            next_start_id = Some(id);
            break;
        }
    }

    Ok(SubscriptionsPage {
        subscription_ids,
        next_start_id,
    })
}

/// Returns all active credit-limit grants issued under `merchant`.
///
/// Reads the `MerchantCreditLimits` secondary index maintained by
/// `do_set_subscriber_credit_limit`. Grants with `limit == 0` (revoked) are
/// excluded; expired grants (where `expires_at <= now`) are also excluded so
/// only currently-enforceable limits are returned.
///
/// Results are in insertion order (oldest grant first). An empty `Vec` means
/// no grants are currently active for this merchant.
///
/// Unauthenticated — credit limit entries are observable from public
/// state; no admin secret or timelock is exposed.
pub fn list_credit_limits(
    env: &Env,
    merchant: Address,
) -> Vec<crate::types::CreditLimitEntry> {
    let now = env.ledger().timestamp();
    let index_key = DataKey::MerchantCreditLimits(merchant);
    let index: Vec<(Address, Address)> = env
        .storage()
        .instance()
        .get(&index_key)
        .unwrap_or(Vec::new(env));

    let mut out = Vec::new(env);
    for (subscriber, token) in index.iter() {
        let key = DataKey::CreditLimit(subscriber.clone(), token.clone());
        if let Some(rec) = env
            .storage()
            .instance()
            .get::<_, crate::types::CreditLimitRecord>(&key)
        {
            // Skip revoked (limit == 0) and expired grants.
            if rec.limit == 0 {
                continue;
            }
            if let Some(exp) = rec.expires_at {
                if now >= exp {
                    continue;
                }
            }
            out.push_back(crate::types::CreditLimitEntry {
                subscriber: subscriber.clone(),
                token: token.clone(),
                limit: rec.limit,
                expires_at: rec.expires_at,
            });
        }
    }
    out
}

/// One page of active credit-limit grants returned by
/// [`list_credit_limits_paginated`].
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreditLimitsPage {
    /// Active grants found in the scanned slice of the index, in index order.
    pub entries: Vec<crate::types::CreditLimitEntry>,
    /// Index offset to resume from on the next call, or `None` once the whole
    /// index has been served.
    pub next_cursor: Option<u32>,
    /// Total number of entries in the `MerchantCreditLimits` index (including
    /// revoked/expired grants), so callers can size their pagination loop.
    pub total: u32,
}

/// Cursor-paginated variant of [`list_credit_limits`].
///
/// The unbounded form reads **every** position of the `MerchantCreditLimits`
/// index in a single call. For a merchant with a very large grant history that
/// exhausts the per-transaction instruction budget and fails with an opaque
/// network error. This variant bounds the work per call: it consumes at most
/// `limit` index positions (and therefore performs at most `limit` record
/// reads), returning a `next_cursor` continuation when more remain.
///
/// # Arguments
///
/// * `cursor` — 0-based offset into the index; `None` starts at the first
///   entry. Pass the returned `next_cursor` to fetch the next page.
/// * `limit` — maximum number of index positions to inspect; must be in
///   `1..=MAX_CREDIT_LIMITS_PAGE`.
///
/// # Returns
///
/// A [`CreditLimitsPage`]. `entries` contains only grants that are currently
/// enforceable (limit `> 0` and not past `expires_at`); revoked or expired
/// entries inside the window are skipped without shrinking the cursor stride,
/// so a page may hold fewer than `limit` entries while `next_cursor` is still
/// `Some`. Continue until `next_cursor` is `None`.
///
/// Unauthenticated — like [`list_credit_limits`], the returned entries are
/// observable from public state.
pub fn list_credit_limits_paginated(
    env: &Env,
    merchant: Address,
    cursor: Option<u32>,
    limit: u32,
) -> Result<CreditLimitsPage, Error> {
    if limit == 0 || limit > MAX_CREDIT_LIMITS_PAGE {
        return Err(Error::InvalidInput);
    }

    let now = env.ledger().timestamp();
    let index_key = DataKey::MerchantCreditLimits(merchant);
    let index: Vec<(Address, Address)> = env
        .storage()
        .instance()
        .get(&index_key)
        .unwrap_or(Vec::new(env));
    let total = index.len();

    let start = cursor.unwrap_or(0);
    if start >= total {
        return Ok(CreditLimitsPage {
            entries: Vec::new(env),
            next_cursor: None,
            total,
        });
    }

    let end = start.saturating_add(limit).min(total);
    let mut entries = Vec::new(env);
    for (subscriber, token) in index
        .iter()
        .skip(start as usize)
        .take((end - start) as usize)
    {
        let key = DataKey::CreditLimit(subscriber.clone(), token.clone());
        if let Some(rec) = env
            .storage()
            .instance()
            .get::<_, crate::types::CreditLimitRecord>(&key)
        {
            // Skip revoked (limit == 0) and expired grants.
            if rec.limit == 0 {
                continue;
            }
            if let Some(exp) = rec.expires_at {
                if now >= exp {
                    continue;
                }
            }
            entries.push_back(crate::types::CreditLimitEntry {
                subscriber: subscriber.clone(),
                token: token.clone(),
                limit: rec.limit,
                expires_at: rec.expires_at,
            });
        }
    }

    let next_cursor = if end < total { Some(end) } else { None };
    Ok(CreditLimitsPage {
        entries,
        next_cursor,
        total,
    })
}

/// One entry of a [`RecoverableSubscriptionsPage`] page.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoverableSubscription {
    pub subscription_id: u32,
    pub subscriber: Address,
    pub merchant: Address,
    pub token: Address,
    /// Non-zero stranded prepaid balance held on the record.
    pub prepaid_balance: i128,
}

/// A page of stranded subscriptions returned by [`list_recoverable_subscriptions`].
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoverableSubscriptionsPage {
    pub subscriptions: Vec<RecoverableSubscription>,
    /// ID to resume the scan from on the next call, or `None` once the scan has
    /// reached the end of the ID space.
    pub next_cursor: Option<u32>,
}

/// Lists subscriptions that are stranded in `Cancelled` status with a non-zero
/// `prepaid_balance`, so admin tooling can surface them without a full chain
/// scan (issue #438).
///
/// # What counts as "stranded"
///
/// A subscription is reported when **both** hold:
///
/// 1. `status == SubscriptionStatus::Cancelled`, and
/// 2. `prepaid_balance > 0`.
///
/// `Cancelled` is normally terminal and its refund is escrowed rather than left
/// on the record — `apply_cancellation` zeroes `prepaid_balance` and moves the
/// amount to `DataKey::CancellationEscrow(subscription_id)`. Rows matching this
/// predicate therefore indicate that funds were added to, or left on, a
/// cancelled record outside that path:
///
/// * a delegated-payer deposit (`deposit_funds_on_behalf`) that lands after
///   cancellation, because that path does not gate on `Cancelled`;
/// * a record restored from a snapshot export whose escrow accounting was not
///   reproduced by the restore;
/// * state migrated from an older storage schema.
///
/// Surfacing these ids is a **forensic/reporting aid only**. It does not grant
/// or imply any right to move the funds: `recover_stranded_funds` remains
/// structurally incapable of touching an accounted balance, and the legitimate
/// route for a cancelled subscription's money is still the subscriber-authenticated
/// `withdraw_subscriber_funds`. See `docs/recovery.md` for the full policy.
///
/// # Pagination
///
/// - `cursor`: inclusive lower bound on the subscription ID to start scanning
///   from. `None` starts at ID `0`.
/// - `limit`: how many matching subscriptions to return; must be in
///   `1..=MAX_QUERY_RESULTS`, otherwise [`Error::InvalidExportLimit`].
///
/// Results are ordered by ascending subscription ID. Each call inspects at most
/// [`MAX_SCAN_DEPTH`] ids; if the budget runs out before `limit` matches are
/// found, `next_cursor` points at the first unscanned ID so the caller can
/// resume. When `next_cursor` is `None` the scan has reached the end of the ID
/// space and enumeration is complete — an empty page with `None` means there is
/// nothing stranded.
///
/// # Complexity
///
/// `O(min(MAX_SCAN_DEPTH, remaining ids))` storage reads per call. A full-ID
/// range scan is structurally required here: `apply_cancellation` removes the
/// id from `MerchantSubs`, `TokenSubs` and `SubscriberSubs`, so a cancelled
/// subscription is not reachable through any secondary index.
///
/// # Safety
///
/// Unauthenticated and unguarded by the emergency stop, like every other view
/// in this module — the returned records are already public state and expose no
/// admin secret or timelock.
pub fn list_recoverable_subscriptions(
    env: &Env,
    cursor: Option<u32>,
    limit: u32,
) -> Result<RecoverableSubscriptionsPage, Error> {
    if limit == 0 || limit > MAX_QUERY_RESULTS {
        return Err(Error::InvalidExportLimit);
    }

    let next_id: u32 = crate::admin::read_config(env, &DataKey::NextId).unwrap_or(0u32);
    let start = cursor.unwrap_or(0);
    if next_id == 0 || start >= next_id {
        return Ok(RecoverableSubscriptionsPage {
            subscriptions: Vec::new(env),
            next_cursor: None,
        });
    }

    let mut subscriptions = Vec::new(env);
    let mut scanned: u32 = 0;
    let mut cursor_id = start;

    while cursor_id < next_id {
        if scanned >= MAX_SCAN_DEPTH {
            break;
        }
        scanned += 1;

        if let Some(sub) = read_subscription(env, cursor_id) {
            if sub.status == SubscriptionStatus::Cancelled && sub.prepaid_balance > 0 {
                if subscriptions.len() < limit {
                    subscriptions.push_back(RecoverableSubscription {
                        subscription_id: cursor_id,
                        subscriber: sub.subscriber,
                        merchant: sub.merchant,
                        token: sub.token,
                        prepaid_balance: sub.prepaid_balance,
                    });
                } else {
                    // Page is full; resume from this ID on the next call.
                    return Ok(RecoverableSubscriptionsPage {
                        subscriptions,
                        next_cursor: Some(cursor_id),
                    });
                }
            }
        }

        cursor_id += 1;
    }

    // Only advertise a resume point when ids remain unscanned. Otherwise the
    // scan reached `next_id` and enumeration is complete.
    let next_cursor = if cursor_id < next_id {
        Some(cursor_id)
    } else {
        None
    };

    Ok(RecoverableSubscriptionsPage {
        subscriptions,
        next_cursor,
    })
}

// ── Reconciliation Queries ───────────────────────────────────────────────────

use crate::types::{
    PrepaidQueryRequest, PrepaidQueryResult, ReconciliationProof, ReconciliationSummaryPage,
    TokenLiabilities,
};

/// Maximum number of subscriptions to scan in a single prepaid balance query.
///
/// This bounds compute to prevent excessive gas usage when aggregating across
/// many subscriptions. Callers should chain paginated calls to build complete totals.
pub const MAX_PREPAID_SCAN_DEPTH: u32 = 500;

/// Maximum number of token summaries to return in a single reconciliation summary call.
pub const MAX_TOKEN_SUMMARIES_PER_PAGE: u32 = 50;

/// Returns complete reconciliation data for a single token.
///
/// This computes the accounting equation:
/// `contract_token_balance = total_prepaid + total_merchant_liabilities + recoverable`
///
/// # Arguments
///
/// * `token` — The settlement token to audit.
///
/// # Returns
///
/// A [`TokenLiabilities`] struct containing all computed values and a balance check.
/// Returns [`Error::InvalidToken`] if `token` is not accepted by the contract.
///
/// # Complexity
///
/// This function scans all subscriptions and all merchants, which can be expensive
/// for large contracts. For bounded compute, use [`query_prepaid_balances_paginated`]
/// and aggregate off-chain.
pub fn get_token_reconciliation(env: &Env, token: Address) -> Result<TokenLiabilities, Error> {
    if !crate::admin::is_token_accepted(env, &token) {
        return Err(Error::InvalidToken);
    }

    let token_client = soroban_sdk::token::Client::new(env, &token);
    let contract_balance = token_client.balance(&env.current_contract_address());

    // Compute total prepaid across all subscriptions
    let total_prepaid = compute_total_prepaid(env, &token);

    // Compute total merchant liabilities using precomputed total_prepaid
    let total_merchant_liabilities =
        compute_total_merchant_liabilities(env, &token, total_prepaid)?;

    // Recoverable is the difference between contract balance and accounted funds
    let accounted = total_prepaid
        .checked_add(total_merchant_liabilities)
        .unwrap_or(0i128);
    let recoverable_amount = contract_balance.saturating_sub(accounted).max(0i128);

    let computed_total = accounted.checked_add(recoverable_amount).unwrap_or(0i128);

    let is_balanced = contract_balance == computed_total;

    let normalized_prepaid =
        crate::types::normalize_amount(env, &token, total_prepaid).unwrap_or(0);
    let normalized_merchant_liab =
        crate::types::normalize_amount(env, &token, total_merchant_liabilities).unwrap_or(0);
    let normalized_recoverable =
        crate::types::normalize_amount(env, &token, recoverable_amount).unwrap_or(0);
    let normalized_contract_balance =
        crate::types::normalize_amount(env, &token, contract_balance).unwrap_or(0);
    let normalized_computed_total =
        crate::types::normalize_amount(env, &token, computed_total).unwrap_or(0);

    Ok(TokenLiabilities {
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
    })
}

/// Returns paginated reconciliation summaries for all accepted tokens.
///
/// # Arguments
///
/// * `start_token_index` — Index into the accepted tokens list to start from.
/// * `limit` — Maximum number of token summaries to return (max 50).
///
/// # Returns
///
/// A [`ReconciliationSummaryPage`] with token summaries and pagination cursor.
pub fn get_contract_reconciliation_summary(
    env: &Env,
    start_token_index: u32,
    limit: u32,
) -> Result<ReconciliationSummaryPage, Error> {
    let limit = limit.min(MAX_TOKEN_SUMMARIES_PER_PAGE);

    // Get all accepted tokens
    let accepted_tokens: Vec<Address> = env
        .storage()
        .instance()
        .get(&DataKey::AcceptedTokens)
        .unwrap_or(Vec::new(env));

    let total_tokens = accepted_tokens.len();

    if start_token_index >= total_tokens {
        return Ok(ReconciliationSummaryPage {
            token_summaries: Vec::new(env),
            next_token_index: None,
        });
    }

    let end_index = (start_token_index + limit).min(total_tokens);
    let mut token_summaries = Vec::new(env);

    for token in accepted_tokens
        .iter()
        .skip(start_token_index as usize)
        .take((end_index - start_token_index) as usize)
    {
        let summary = get_token_reconciliation(env, token)?;
        token_summaries.push_back(summary);
    }

    let next_token_index = if end_index < total_tokens {
        Some(end_index)
    } else {
        None
    };

    Ok(ReconciliationSummaryPage {
        token_summaries,
        next_token_index,
    })
}

/// Generates an auditable proof for off-chain reconciliation verification.
///
/// This function creates a snapshot with all necessary data for auditors to
/// independently validate the accounting equation without needing full
/// contract state access.
///
/// # Arguments
///
/// * `token` — The settlement token to generate the proof for.
///
/// # Returns
///
/// A [`ReconciliationProof`] containing all validation data.
/// Returns [`Error::InvalidToken`] if `token` is not accepted by the contract.
///
/// # Security
///
/// This is a read-only function that cannot modify state. The proof is generated
/// at the current ledger state and includes the ledger sequence for temporal
/// anchoring.
pub fn generate_reconciliation_proof(
    env: &Env,
    token: Address,
) -> Result<ReconciliationProof, Error> {
    if !crate::admin::is_token_accepted(env, &token) {
        return Err(Error::InvalidToken);
    }

    let token_client = soroban_sdk::token::Client::new(env, &token);
    let contract_balance = token_client.balance(&env.current_contract_address());

    // Get prepaid total with count
    let (total_prepaid, sub_count) = compute_total_prepaid_with_count(env, &token);

    // Get merchant liabilities with count
    let (total_merchant_liabilities, merchant_count) =
        compute_total_merchant_liabilities_with_count(env, &token, total_prepaid)?;

    // Compute recoverable
    let accounted = total_prepaid
        .checked_add(total_merchant_liabilities)
        .unwrap_or(0i128);
    let computed_recoverable = contract_balance.saturating_sub(accounted).max(0i128);

    // Validate accounting equation
    let computed_total = accounted.checked_add(computed_recoverable).unwrap_or(0i128);
    let is_valid = contract_balance == computed_total;

    Ok(ReconciliationProof {
        timestamp: env.ledger().timestamp(),
        ledger_sequence: env.ledger().sequence(),
        token,
        contract_balance,
        total_prepaid,
        total_merchant_liabilities,
        computed_recoverable,
        subscription_count: sub_count,
        merchant_count,
        is_valid,
    })
}

/// Returns paginated prepaid balance aggregation for a token.
///
/// This provides bounded compute for auditors to incrementally build the total
/// prepaid balance without iterating unbounded subscription sets in a single call.
///
/// # Arguments
///
/// * `request` — A [`PrepaidQueryRequest`] specifying token, start ID, and scan limit.
///
/// # Returns
///
/// A [`PrepaidQueryResult`] with partial totals and pagination info.
///
/// # Example
///
/// To compute the full prepaid total off-chain:
/// 1. Call with `start_subscription_id = 0`
/// 2. Sum `partial_total` from each response
/// 3. Use `next_start_id` for subsequent calls until `has_more` is false
pub fn query_prepaid_balances_paginated(
    env: &Env,
    request: PrepaidQueryRequest,
) -> PrepaidQueryResult {
    let scan_limit = request.scan_limit.min(MAX_PREPAID_SCAN_DEPTH);
    let next_id: u32 = crate::admin::read_config(env, &DataKey::NextId).unwrap_or(0u32);

    if next_id == 0 || request.start_subscription_id >= next_id {
        return PrepaidQueryResult {
            token: request.token,
            partial_total: 0,
            subscriptions_count: 0,
            next_start_id: None,
            has_more: false,
        };
    }

    let scan_end = (request.start_subscription_id + scan_limit).min(next_id);
    let mut partial_total: i128 = 0;
    let mut subscriptions_count: u32 = 0;

    for id in request.start_subscription_id..scan_end {
        if let Some(sub) = read_subscription(env, id) {
            if sub.token == request.token && sub.prepaid_balance > 0 {
                partial_total = partial_total.saturating_add(sub.prepaid_balance);
                subscriptions_count = subscriptions_count.saturating_add(1);
            }
        }
    }

    let has_more = scan_end < next_id;
    let next_start_id = if has_more { Some(scan_end) } else { None };

    PrepaidQueryResult {
        token: request.token,
        partial_total,
        subscriptions_count,
        next_start_id,
        has_more,
    }
}

// ── Bounded (cursor-paginated) reconciliation variants ──────────────────────
//
// The entrypoints above that return a complete reconciliation result
// (`get_token_reconciliation`, `generate_reconciliation_proof`,
// `get_contract_reconciliation_summary`) each perform a full `0..NextId`
// subscription scan. On a large contract that scan exhausts the per-transaction
// instruction budget and the call fails with an opaque network error, which
// breaks off-chain reconciliation pipelines. The `*_paginated` variants below
// bound the work per call to at most `MAX_PREPAID_SCAN_DEPTH` subscription reads
// and return a continuation cursor; callers accumulate the partial totals.

/// One bounded page of a token-level reconciliation scan.
///
/// Returned by [`get_token_reconciliation_paginated`] and
/// [`generate_reconciliation_proof_paginated`]. Each page covers at most
/// `MAX_PREPAID_SCAN_DEPTH` subscription IDs; sum `partial_prepaid` across pages
/// (following `next_cursor`) to rebuild the token's full prepaid total.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TokenReconciliationPage {
    pub token: Address,
    /// Ledger timestamp at which this page was produced.
    pub timestamp: u64,
    /// Ledger sequence at which this page was produced.
    pub ledger_sequence: u32,
    /// Contract's current token balance — an O(1) read, not part of the scan.
    pub contract_balance: i128,
    /// Prepaid balance summed over the scanned subscription window only.
    pub partial_prepaid: i128,
    /// Subscriptions with a non-zero prepaid balance found in the window.
    pub subscriptions_count: u32,
    /// Subscription ID to resume from on the next call, or `None` when the scan
    /// has reached the end of the ID space.
    pub next_cursor: Option<u32>,
    /// `true` while `next_cursor` is `Some` (more IDs remain unscanned).
    pub has_more: bool,
}

/// Cursor-paginated variant of [`get_token_reconciliation`].
///
/// The unbounded form scans every subscription ID up to `NextId`, which fails on
/// large contracts. This variant scans at most `MAX_PREPAID_SCAN_DEPTH` IDs per
/// call and returns a partial prepaid subtotal plus a `next_cursor`. Accumulate
/// `partial_prepaid` across pages until `has_more` is `false`, then apply the
/// accounting equation off-chain.
///
/// Returns [`Error::InvalidToken`] if `token` is not accepted by the contract.
///
/// `scan_limit` is clamped to `1..=MAX_PREPAID_SCAN_DEPTH`; it controls the size
/// of the scanned ID window, not the number of results.
pub fn get_token_reconciliation_paginated(
    env: &Env,
    token: Address,
    cursor: Option<u32>,
    scan_limit: u32,
) -> Result<TokenReconciliationPage, Error> {
    if !crate::admin::is_token_accepted(env, &token) {
        return Err(Error::InvalidToken);
    }

    let limit = scan_limit.clamp(1, MAX_PREPAID_SCAN_DEPTH);
    let token_client = soroban_sdk::token::Client::new(env, &token);
    let contract_balance = token_client.balance(&env.current_contract_address());

    let (partial_prepaid, subscriptions_count, next_cursor) =
        scan_prepaid_window(env, &token, cursor.unwrap_or(0), limit);

    Ok(TokenReconciliationPage {
        token,
        timestamp: env.ledger().timestamp(),
        ledger_sequence: env.ledger().sequence(),
        contract_balance,
        partial_prepaid,
        subscriptions_count,
        next_cursor,
        has_more: next_cursor.is_some(),
    })
}

/// One bounded page of an auditable reconciliation proof.
///
/// Returned by [`generate_reconciliation_proof_paginated`]. Same bounded-scan
/// model as [`TokenReconciliationPage`], but each page carries the ledger
/// anchoring (`timestamp` / `ledger_sequence`) so auditors can pin every chunk
/// to a ledger state.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReconciliationProofPage {
    /// Ledger timestamp at which this page was produced.
    pub timestamp: u64,
    /// Ledger sequence at which this page was produced.
    pub ledger_sequence: u32,
    pub token: Address,
    /// Contract's current token balance — an O(1) read, not part of the scan.
    pub contract_balance: i128,
    /// Prepaid balance summed over the scanned subscription window only.
    pub partial_prepaid: i128,
    /// Subscriptions with a non-zero prepaid balance found in the window.
    pub subscription_count: u32,
    /// Subscription ID to resume from on the next call, or `None` when the scan
    /// has reached the end of the ID space.
    pub next_cursor: Option<u32>,
    /// `true` while `next_cursor` is `Some` (more IDs remain unscanned).
    pub has_more: bool,
}

/// Cursor-paginated variant of [`generate_reconciliation_proof`].
///
/// Identical bounded-scan model to [`get_token_reconciliation_paginated`] but
/// carries the ledger anchoring (`timestamp` / `ledger_sequence`) on each page so
/// auditors can pin every chunk to a ledger state. Accumulate `partial_prepaid`
/// across pages until `has_more` is `false` to rebuild the token's total.
///
/// Returns [`Error::InvalidToken`] if `token` is not accepted by the contract.
pub fn generate_reconciliation_proof_paginated(
    env: &Env,
    token: Address,
    cursor: Option<u32>,
    scan_limit: u32,
) -> Result<ReconciliationProofPage, Error> {
    if !crate::admin::is_token_accepted(env, &token) {
        return Err(Error::InvalidToken);
    }

    let limit = scan_limit.clamp(1, MAX_PREPAID_SCAN_DEPTH);
    let token_client = soroban_sdk::token::Client::new(env, &token);
    let contract_balance = token_client.balance(&env.current_contract_address());

    let (partial_prepaid, subscription_count, next_cursor) =
        scan_prepaid_window(env, &token, cursor.unwrap_or(0), limit);

    Ok(ReconciliationProofPage {
        timestamp: env.ledger().timestamp(),
        ledger_sequence: env.ledger().sequence(),
        token,
        contract_balance,
        partial_prepaid,
        subscription_count,
        next_cursor,
        has_more: next_cursor.is_some(),
    })
}

/// A partial (bounded) reconciliation summary for a single token.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TokenPartialSummary {
    pub token: Address,
    /// Prepaid balance summed over the scanned subscription window only.
    pub partial_prepaid: i128,
    /// Subscriptions with a non-zero prepaid balance found in the window.
    pub subscriptions_count: u32,
}

/// A bounded page of the multi-token reconciliation summary.
///
/// Returned by [`get_contract_reconciliation_summary_paginated`].
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReconciliationSummaryScanPage {
    /// Partial summaries for the tokens in the requested token window.
    pub token_summaries: Vec<TokenPartialSummary>,
    /// Token index to advance to once the subscription scan for this token
    /// window is complete (`next_subscription_cursor == None`).
    pub next_token_index: Option<u32>,
    /// Subscription ID to resume the scan from for the **current** token window,
    /// or `None` when the window has been fully scanned.
    pub next_subscription_cursor: Option<u32>,
}

/// Cursor-paginated variant of [`get_contract_reconciliation_summary`].
///
/// The unbounded form calls [`get_token_reconciliation`] for each token in the
/// window, and each of those performs a full `0..NextId` subscription scan. This
/// variant instead performs a single bounded subscription scan (at most
/// `MAX_PREPAID_SCAN_DEPTH` reads in total, shared evenly across the tokens in
/// the window) and returns per-token partial prepaid subtotals.
///
/// # Two-level iteration (off-chain)
///
/// 1. Start with `start_token_index = 0`, a `limit`, and
///    `subscription_cursor = None`.
/// 2. Call repeatedly, feeding the returned `next_subscription_cursor` back in
///    and accumulating each token's `partial_prepaid`, until the returned
///    `next_subscription_cursor` is `None`. At that point the token window has
///    been fully scanned.
/// 3. Advance to the returned `next_token_index` (if `Some`) and repeat from
///    step 2 with `subscription_cursor = None`.
///
/// `limit == 0` is rejected with [`Error::InvalidInput`].
pub fn get_contract_reconciliation_summary_paginated(
    env: &Env,
    start_token_index: u32,
    limit: u32,
    subscription_cursor: Option<u32>,
) -> Result<ReconciliationSummaryScanPage, Error> {
    let token_count = limit.min(MAX_TOKEN_SUMMARIES_PER_PAGE);
    if token_count == 0 {
        return Err(Error::InvalidInput);
    }

    let accepted_tokens: Vec<Address> = env
        .storage()
        .instance()
        .get(&DataKey::AcceptedTokens)
        .unwrap_or(Vec::new(env));
    let total_tokens = accepted_tokens.len();

    if start_token_index >= total_tokens {
        return Ok(ReconciliationSummaryScanPage {
            token_summaries: Vec::new(env),
            next_token_index: None,
            next_subscription_cursor: None,
        });
    }

    let end_index = start_token_index.saturating_add(token_count).min(total_tokens);
    let next_token_index = if end_index < total_tokens {
        Some(end_index)
    } else {
        None
    };

    // The per-call read budget is shared across the tokens in the window so the
    // total subscription reads never exceed `MAX_PREPAID_SCAN_DEPTH`.
    let next_id: u32 = crate::admin::read_config(env, &DataKey::NextId).unwrap_or(0u32);
    let start_sub = subscription_cursor.unwrap_or(0);
    let per_token_budget = (MAX_PREPAID_SCAN_DEPTH / token_count).max(1);
    let scan_end = if next_id == 0 || start_sub >= next_id {
        start_sub
    } else {
        start_sub.saturating_add(per_token_budget).min(next_id)
    };
    let next_subscription_cursor = if scan_end < next_id {
        Some(scan_end)
    } else {
        None
    };
    let scan_window = scan_end.saturating_sub(start_sub);

    let mut token_summaries = Vec::new(env);
    for token in accepted_tokens
        .iter()
        .skip(start_token_index as usize)
        .take((end_index - start_token_index) as usize)
    {
        let (partial_prepaid, subscriptions_count, _) =
            scan_prepaid_window(env, &token, start_sub, scan_window);
        token_summaries.push_back(TokenPartialSummary {
            token,
            partial_prepaid,
            subscriptions_count,
        });
    }

    Ok(ReconciliationSummaryScanPage {
        token_summaries,
        next_token_index,
        next_subscription_cursor,
    })
}

// ── Internal helpers for reconciliation ────────────────────────────────────

/// Scans a bounded window `[start, start + scan_limit)` of subscription IDs and
/// returns the `(prepaid subtotal, matching count, next_cursor)` for `token`.
///
/// `next_cursor` is `Some(id)` when unscanned IDs remain (so the caller can
/// resume), or `None` when the window reached `NextId`. Prepaid balances are
/// summed only when non-zero, matching [`query_prepaid_balances_paginated`].
fn scan_prepaid_window(
    env: &Env,
    token: &Address,
    start: u32,
    scan_limit: u32,
) -> (i128, u32, Option<u32>) {
    let next_id: u32 = crate::admin::read_config(env, &DataKey::NextId).unwrap_or(0u32);
    if next_id == 0 || start >= next_id || scan_limit == 0 {
        return (0, 0, None);
    }

    let scan_end = start.saturating_add(scan_limit).min(next_id);
    let mut total: i128 = 0;
    let mut count: u32 = 0;
    for id in start..scan_end {
        if let Some(sub) = read_subscription(env, id) {
            if sub.token == *token && sub.prepaid_balance > 0 {
                total = total.saturating_add(sub.prepaid_balance);
                count = count.saturating_add(1);
            }
        }
    }

    let next_cursor = if scan_end < next_id {
        Some(scan_end)
    } else {
        None
    };
    (total, count, next_cursor)
}

fn compute_total_prepaid(env: &Env, token: &Address) -> i128 {
    let next_id: u32 = crate::admin::read_config(env, &DataKey::NextId).unwrap_or(0u32);

    let mut total: i128 = 0;
    for id in 0..next_id {
        if let Some(sub) = read_subscription(env, id) {
            if sub.token == *token {
                total = total.saturating_add(sub.prepaid_balance);
            }
        }
    }
    total
}

fn compute_total_prepaid_with_count(env: &Env, token: &Address) -> (i128, u32) {
    let next_id: u32 = crate::admin::read_config(env, &DataKey::NextId).unwrap_or(0u32);

    let mut total: i128 = 0;
    let mut count: u32 = 0;
    for id in 0..next_id {
        if let Some(sub) = read_subscription(env, id) {
            if sub.token == *token && sub.prepaid_balance > 0 {
                total = total.saturating_add(sub.prepaid_balance);
                count = count.saturating_add(1);
            }
        }
    }
    (total, count)
}

fn compute_total_merchant_liabilities(
    env: &Env,
    token: &Address,
    total_prepaid: i128,
) -> Result<i128, Error> {
    let total_accounted = crate::accounting::get_total_accounted(env, token)?;
    Ok(total_accounted.saturating_sub(total_prepaid).max(0i128))
}

fn compute_total_merchant_liabilities_with_count(
    env: &Env,
    token: &Address,
    total_prepaid: i128,
) -> Result<(i128, u32), Error> {
    let total_accounted = crate::accounting::get_total_accounted(env, token)?;
    let total = total_accounted.saturating_sub(total_prepaid).max(0i128);

    let mut merchant_count: u32 = 0;
    if total > 0 {
        merchant_count = 1;
    }

    Ok((total, merchant_count))
}
