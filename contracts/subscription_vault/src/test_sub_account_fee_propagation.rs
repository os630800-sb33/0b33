//! Comprehensive tests for sub-account fee propagation and nesting verification.
//!
//! This module verifies:
//! 1. Fee routing is deterministic and applied exactly once per charge
//! 2. Sub-accounts are single-level (not nested)
//! 3. Fee conservation invariants hold at each routing level
//! 4. No double-charging or fee leakage occurs

#[cfg(test)]
mod fee_propagation_tests {
    use crate::{Error, SubscriptionVault, SubscriptionVaultClient};
    use soroban_sdk::testutils::{Address as _, Events as _};
    use soroban_sdk::{Address, Env, Symbol, Vec};

    fn label(env: &Env, s: &str) -> Symbol {
        Symbol::new(env, s)
    }

    fn setup() -> (Env, SubscriptionVaultClient<'static>, Address, Address) {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let token = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
        client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));
        (env, client, admin, token)
    }

    fn create_merchant(
        client: &SubscriptionVaultClient<'static>,
        env: &Env,
    ) -> Address {
        let merchant = Address::generate(env);
        let payout = Address::generate(env);
        client.initialize_merchant_config(
            &merchant,
            &payout,
            &0,
            &1,
            &None,
            &soroban_sdk::String::from_str(env, ""),
        );
        merchant
    }

    // ───────────────────────────────────────────────────────────────────────────────
    // Test: Fee Applied Exactly Once per Charge to Sub-Account
    // ───────────────────────────────────────────────────────────────────────────────

    /// Verify that when a subscription charges to a sub-account, the fee is applied
    /// exactly once, not per-level or per-routing-step.
    #[test]
    fn fee_applied_once_to_sub_account_charge() {
        let (env, client, admin, token) = setup();
        let merchant = create_merchant(&client, &env);

        // Setup: register a sub-account
        let sales_label = label(&env, "sales");
        client.register_sub_account(&merchant, &sales_label);

        // Setup: set a non-zero protocol fee (e.g., 250 bps = 2.5%)
        client.set_protocol_fee(&admin, &250);

        // Setup: create a subscriber with funds
        client.set_min_topup(&admin, &1);
        let subscriber = Address::generate(&env);

        let stellar = soroban_sdk::token::StellarAssetClient::new(&env, &token);
        stellar.mint(&subscriber, &100_000_000); // 100 USDC in 6-decimal

        // Create subscription charging 1_000_000 (1 USDC) to the sub-account
        let sub_id = client
            .create_subscription_with_token(
                &subscriber,
                &merchant,
                &token,
                &1_000_000, // 1 USDC
                &86400,
                &false,
                &None::<i128>,
                &None::<u64>,
                &None::<u32>,
                &Some(sales_label.clone()),
            )
            .unwrap();

        client.deposit_funds(&sub_id, &5_000_000, &None); // 5 USDC
        client.charge_subscription(&sub_id, &None);

        // Verify: Sub-account receives net amount (after one fee deduction)
        // Gross: 1_000_000
        // Fee (2.5%): 1_000_000 * 250 / 10_000 = 25_000
        // Net: 1_000_000 - 25_000 = 975_000
        let sub_account_balance = client.get_sub_account_balance(&merchant, &sales_label);
        assert_eq!(
            sub_account_balance, 975_000,
            "Sub-account should receive net after one fee deduction"
        );

        // Verify: Parent earnings reflect the gross charge (for audit)
        let earnings = client.get_merchant_token_earnings(&merchant, &token);
        assert_eq!(
            earnings.accruals.interval, 1_000_000,
            "Parent earnings should record gross charge"
        );

        // Verify: Sub-account balance breakdown
        let breakdown = client.get_merchant_token_earnings_with_sub_accounts(&merchant, &token);
        assert_eq!(
            breakdown.sub_account_balances.len(),
            1,
            "Should have exactly one sub-account"
        );
        assert_eq!(
            breakdown.sub_account_balances[0].balance, 975_000,
            "Sub-account breakdown should match queried balance"
        );
        assert_eq!(
            breakdown.total_balance, 975_000,
            "Total balance should equal sub-account (primary empty)"
        );
    }

    // ───────────────────────────────────────────────────────────────────────────────
    // Test: Fee Conservation Invariant
    // ───────────────────────────────────────────────────────────────────────────────

    /// Verify the invariant: gross == net + fee, for multiple sub-account charges
    #[test]
    fn fee_conservation_invariant_holds() {
        let (env, client, admin, token) = setup();
        let merchant = create_merchant(&client, &env);

        let sales = label(&env, "sales");
        let support = label(&env, "support");
        client.register_sub_account(&merchant, &sales);
        client.register_sub_account(&merchant, &support);

        client.set_protocol_fee(&admin, &333); // 3.33%
        client.set_min_topup(&admin, &1);

        let subscriber = Address::generate(&env);
        let stellar = soroban_sdk::token::StellarAssetClient::new(&env, &token);
        stellar.mint(&subscriber, &1_000_000_000);

        // Charge 1: 100 USDC to sales
        let sub_id_1 = client
            .create_subscription_with_token(
                &subscriber,
                &merchant,
                &token,
                &100_000_000,
                &86400,
                &false,
                &None::<i128>,
                &None::<u64>,
                &None::<u32>,
                &Some(sales.clone()),
            )
            .unwrap();
        client.deposit_funds(&sub_id_1, &100_000_000, &None);
        client.charge_subscription(&sub_id_1, &None);

        // Charge 2: 50 USDC to support
        let sub_id_2 = client
            .create_subscription_with_token(
                &subscriber,
                &merchant,
                &token,
                &50_000_000,
                &86400,
                &false,
                &None::<i128>,
                &None::<u64>,
                &None::<u32>,
                &Some(support.clone()),
            )
            .unwrap();
        client.deposit_funds(&sub_id_2, &50_000_000, &None);
        client.charge_subscription(&sub_id_2, &None);

        // Verify conservation for each sub-account
        let sales_balance = client.get_sub_account_balance(&merchant, &sales);
        let support_balance = client.get_sub_account_balance(&merchant, &support);

        // Charge 1: 100_000_000 * 333 / 10_000 = 3_330_000
        let fee_1 = 100_000_000 * 333 / 10_000;
        let net_1 = 100_000_000 - fee_1;
        assert_eq!(
            sales_balance, net_1,
            "Sales balance should be gross - fee"
        );

        // Charge 2: 50_000_000 * 333 / 10_000 = 1_665_000
        let fee_2 = 50_000_000 * 333 / 10_000;
        let net_2 = 50_000_000 - fee_2;
        assert_eq!(
            support_balance, net_2,
            "Support balance should be gross - fee"
        );

        // Verify parent earnings reflect both charges (gross)
        let earnings = client.get_merchant_token_earnings(&merchant, &token);
        assert_eq!(
            earnings.accruals.interval, 100_000_000 + 50_000_000,
            "Parent earnings should record all charges at gross"
        );

        // Verify breakdown totals
        let breakdown = client.get_merchant_token_earnings_with_sub_accounts(&merchant, &token);
        assert_eq!(
            breakdown.total_balance,
            sales_balance + support_balance,
            "Total balance should equal sum of sub-accounts"
        );
        assert_eq!(
            breakdown.primary_balance, 0,
            "Primary balance should be zero (all funds in sub-accounts)"
        );
    }

    // ───────────────────────────────────────────────────────────────────────────────
    // Test: Minimum Fee Floor (Dust)
    // ───────────────────────────────────────────────────────────────────────────────

    /// Verify that even for very small charges to sub-accounts, the minimum fee floor
    /// (1 base unit) is applied when configured, ensuring no protocol fee leakage.
    #[test]
    fn minimum_fee_floor_applied_to_sub_account() {
        let (env, client, admin, token) = setup();
        let merchant = create_merchant(&client, &env);

        let sales = label(&env, "sales");
        client.register_sub_account(&merchant, &sales);

        client.set_protocol_fee(&admin, &250); // 2.5%, even on micro-charges
        client.set_min_topup(&admin, &1);

        let subscriber = Address::generate(&env);
        let stellar = soroban_sdk::token::StellarAssetClient::new(&env, &token);
        stellar.mint(&subscriber, &1_000); // Small amount

        // Create subscription charging 1 unit (smallest possible charge)
        let sub_id = client
            .create_subscription_with_token(
                &subscriber,
                &merchant,
                &token,
                &1,
                &86400,
                &false,
                &None::<i128>,
                &None::<u64>,
                &None::<u32>,
                &Some(sales.clone()),
            )
            .unwrap();

        client.deposit_funds(&sub_id, &100, &None);
        client.charge_subscription(&sub_id, &None);

        // Verify: Even on a 1-unit charge, the fee floor ensures 1 unit goes to treasury
        // Gross: 1
        // Fee calculation: 1 * 250 / 10_000 = 0 (floor)
        // But with floor: max(0, 1) = 1
        // Net: 1 - 1 = 0
        let sub_balance = client.get_sub_account_balance(&merchant, &sales);
        assert_eq!(
            sub_balance, 0,
            "Sub-account receives 0 (full unit taken by fee floor)"
        );

        // Verify parent earnings still see the gross
        let earnings = client.get_merchant_token_earnings(&merchant, &token);
        assert_eq!(
            earnings.accruals.interval, 1,
            "Parent earnings record gross even when fee floor applies"
        );
    }

    // ───────────────────────────────────────────────────────────────────────────────
    // Test: Nested Sub-Accounts Are Not Supported
    // ───────────────────────────────────────────────────────────────────────────────

    /// Verify that attempting to route to nested sub-accounts (sub-account of a sub-account)
    /// is not supported and fails cleanly.
    #[test]
    fn nested_sub_account_not_supported() {
        let (env, client, admin, token) = setup();
        let merchant = create_merchant(&client, &env);

        // Setup parent sub-account
        let sales = label(&env, "sales");
        client.register_sub_account(&merchant, &sales);

        // Attempt to register a sub-account on a sub-account (should fail)
        // Note: The API does not accept a parent_sub_account parameter, so this test
        // verifies that there is no mechanism to create nested structures.
        //
        // The register_sub_account function only takes (merchant, label).
        // There is no "parent_sub_account" parameter, so nesting is architecturally
        // impossible without a contract upgrade.

        // Verify: We can only register sub-accounts on merchants, not on labels
        let nested_label = label(&env, "team");

        // This would fail with Error::NotFound if attempted:
        // client.register_sub_account(&sales, &nested_label);
        // Because register_sub_account requires merchant authorization (Address),
        // and a Symbol (label) is not an Address.

        // Instead, verify the architectural constraint by attempting to charge to
        // a hierarchical label name (which would fail at charge time).
        let hierarchical_label = label(&env, "sales/team");

        // Create subscription with hierarchical label (compiles, but...)
        let subscriber = Address::generate(&env);
        let stellar = soroban_sdk::token::StellarAssetClient::new(&env, &token);
        stellar.mint(&subscriber, &10_000);

        let sub_id = client
            .create_subscription_with_token(
                &subscriber,
                &merchant,
                &token,
                &1000,
                &86400,
                &false,
                &None::<i128>,
                &None::<u64>,
                &None::<u32>,
                &Some(hierarchical_label.clone()),
            )
            .unwrap();

        client.set_min_topup(&admin, &1);
        client.deposit_funds(&sub_id, &5000, &None);

        // ... fails at charge time with Error::NotFound because no such sub-account exists
        let result = client.try_charge_subscription(&sub_id, &None);
        assert_eq!(
            result,
            Err(Ok(Error::NotFound)),
            "Hierarchical label fails cleanly when not registered as exact sub-account"
        );
    }

    // ───────────────────────────────────────────────────────────────────────────────
    // Test: No Double-Charging in Single-Level Sub-Account Routing
    // ───────────────────────────────────────────────────────────────────────────────

    /// Verify that no double-charging occurs when a subscription is charged to a sub-account.
    /// The gross is deducted from prepaid exactly once, and the fee is applied exactly once.
    #[test]
    fn no_double_charging_single_level_routing() {
        let (env, client, admin, token) = setup();
        let merchant = create_merchant(&client, &env);

        let sales = label(&env, "sales");
        client.register_sub_account(&merchant, &sales);

        client.set_protocol_fee(&admin, &1000); // 10% for easy verification
        client.set_min_topup(&admin, &1);

        let subscriber = Address::generate(&env);
        let stellar = soroban_sdk::token::StellarAssetClient::new(&env, &token);
        stellar.mint(&subscriber, &10_000_000);

        // Create subscription
        let sub_id = client
            .create_subscription_with_token(
                &subscriber,
                &merchant,
                &token,
                &1_000_000, // 1 unit in base decimals
                &86400,
                &false,
                &None::<i128>,
                &None::<u64>,
                &None::<u32>,
                &Some(sales.clone()),
            )
            .unwrap();

        client.deposit_funds(&sub_id, &5_000_000, &None);

        // Before charge
        let subscription_before = client.get_subscription(&sub_id);
        let prepaid_before = subscription_before.prepaid_balance;
        let sub_balance_before = client.get_sub_account_balance(&merchant, &sales);

        // Charge
        client.charge_subscription(&sub_id, &None);

        // After charge
        let subscription_after = client.get_subscription(&sub_id);
        let prepaid_after = subscription_after.prepaid_balance;
        let sub_balance_after = client.get_sub_account_balance(&merchant, &sales);

        // Verify: Prepaid deducted exactly once (not twice)
        let prepaid_deducted = prepaid_before - prepaid_after;
        assert_eq!(
            prepaid_deducted, 1_000_000,
            "Prepaid deducted exactly once per charge"
        );

        // Verify: Sub-account credited with net (after single fee)
        let expected_fee = 1_000_000 * 1000 / 10_000; // 100_000 (10%)
        let expected_net = 1_000_000 - expected_fee; // 900_000
        let sub_balance_credited = sub_balance_after - sub_balance_before;
        assert_eq!(
            sub_balance_credited, expected_net,
            "Sub-account credited with net after single fee (no double-charging)"
        );
    }

    // ───────────────────────────────────────────────────────────────────────────────
    // Test: Multiple Charges to Same Sub-Account Accumulate Correctly
    // ───────────────────────────────────────────────────────────────────────────────

    /// Verify that multiple charges to the same sub-account accumulate without fee interference.
    #[test]
    fn multiple_charges_to_same_sub_account_accumulate() {
        let (env, client, admin, token) = setup();
        let merchant = create_merchant(&client, &env);

        let sales = label(&env, "sales");
        client.register_sub_account(&merchant, &sales);

        client.set_protocol_fee(&admin, &500); // 5%
        client.set_min_topup(&admin, &1);

        let subscriber = Address::generate(&env);
        let stellar = soroban_sdk::token::StellarAssetClient::new(&env, &token);
        stellar.mint(&subscriber, &100_000_000);

        // Create two subscriptions charging to the same sub-account
        let sub_id_1 = client
            .create_subscription_with_token(
                &subscriber,
                &merchant,
                &token,
                &1_000_000,
                &86400,
                &false,
                &None::<i128>,
                &None::<u64>,
                &None::<u32>,
                &Some(sales.clone()),
            )
            .unwrap();

        let sub_id_2 = client
            .create_subscription_with_token(
                &subscriber,
                &merchant,
                &token,
                &2_000_000,
                &86400,
                &false,
                &None::<i128>,
                &None::<u64>,
                &None::<u32>,
                &Some(sales.clone()),
            )
            .unwrap();

        client.deposit_funds(&sub_id_1, &10_000_000, &None);
        client.deposit_funds(&sub_id_2, &20_000_000, &None);

        // Charge first subscription
        client.charge_subscription(&sub_id_1, &None);
        let balance_after_1 = client.get_sub_account_balance(&merchant, &sales);

        // Charge second subscription
        client.charge_subscription(&sub_id_2, &None);
        let balance_after_2 = client.get_sub_account_balance(&merchant, &sales);

        // Verify: Balances accumulate
        // Charge 1: 1M * 95% = 950k
        // Charge 2: 2M * 95% = 1.9M
        // Total: 2.85M
        let expected_net_1 = 1_000_000 - 1_000_000 * 500 / 10_000;
        let expected_net_2 = 2_000_000 - 2_000_000 * 500 / 10_000;
        let expected_total = expected_net_1 + expected_net_2;

        assert_eq!(
            balance_after_1, expected_net_1,
            "Sub-account balance after first charge"
        );
        assert_eq!(
            balance_after_2, expected_total,
            "Sub-account balance accumulates correctly"
        );

        // Verify parent earnings reflect both gross charges
        let earnings = client.get_merchant_token_earnings(&merchant, &token);
        assert_eq!(
            earnings.accruals.interval,
            1_000_000 + 2_000_000,
            "Parent earnings accumulate all gross charges"
        );
    }

    // ───────────────────────────────────────────────────────────────────────────────
    // Test: Withdrawal from Sub-Account Does Not Double Fee
    // ───────────────────────────────────────────────────────────────────────────────

    /// Verify that withdrawing from a sub-account correctly debits the balance without
    /// re-applying fees or triggering hidden charges.
    #[test]
    fn withdrawal_from_sub_account_no_double_fee() {
        let (env, client, admin, token) = setup();
        let merchant = create_merchant(&client, &env);

        let sales = label(&env, "sales");
        client.register_sub_account(&merchant, &sales);

        client.set_protocol_fee(&admin, &250);
        client.set_min_topup(&admin, &1);

        // Setup: manually credit sub-account with known amount (post-fee)
        let stellar = soroban_sdk::token::StellarAssetClient::new(&env, &token);
        let contract = env.current_contract_address();
        stellar.mint(&contract, &100_000);

        crate::merchant::credit_sub_account(&env, &merchant, &sales, &token, 50_000).unwrap();

        let balance_before = client.get_sub_account_balance(&merchant, &sales);
        assert_eq!(balance_before, 50_000);

        // Withdraw part of the sub-account
        client.withdraw_sub_account_funds(&merchant, &sales, &token, &10_000);

        let balance_after = client.get_sub_account_balance(&merchant, &sales);
        assert_eq!(
            balance_after, 40_000,
            "Withdrawal deducts from balance without re-applying fees"
        );

        // Verify parent earnings withdrawal is recorded
        let earnings = client.get_merchant_token_earnings(&merchant, &token);
        assert_eq!(
            earnings.withdrawals, 10_000,
            "Withdrawal recorded in parent earnings"
        );
    }

    // ───────────────────────────────────────────────────────────────────────────────
    // Test: Zero Fee (fee_bps = 0) To Sub-Account
    // ───────────────────────────────────────────────────────────────────────────────

    /// Verify that when fee is disabled (fee_bps = 0), all gross goes to sub-account.
    #[test]
    fn zero_fee_all_gross_to_sub_account() {
        let (env, client, admin, token) = setup();
        let merchant = create_merchant(&client, &env);

        let sales = label(&env, "sales");
        client.register_sub_account(&merchant, &sales);

        client.set_protocol_fee(&admin, &0); // No fee
        client.set_min_topup(&admin, &1);

        let subscriber = Address::generate(&env);
        let stellar = soroban_sdk::token::StellarAssetClient::new(&env, &token);
        stellar.mint(&subscriber, &10_000_000);

        let sub_id = client
            .create_subscription_with_token(
                &subscriber,
                &merchant,
                &token,
                &1_000_000,
                &86400,
                &false,
                &None::<i128>,
                &None::<u64>,
                &None::<u32>,
                &Some(sales.clone()),
            )
            .unwrap();

        client.deposit_funds(&sub_id, &5_000_000, &None);
        client.charge_subscription(&sub_id, &None);

        let balance = client.get_sub_account_balance(&merchant, &sales);
        assert_eq!(
            balance, 1_000_000,
            "Sub-account receives full gross when fee is 0"
        );
    }

    // ───────────────────────────────────────────────────────────────────────────────
    // Test: Max Fee (fee_bps = 10000) To Sub-Account
    // ───────────────────────────────────────────────────────────────────────────────

    /// Verify that when fee is maxed (fee_bps = 10000, i.e., 100%), nothing goes to sub-account.
    #[test]
    fn max_fee_nothing_to_sub_account() {
        let (env, client, admin, token) = setup();
        let merchant = create_merchant(&client, &env);

        let sales = label(&env, "sales");
        client.register_sub_account(&merchant, &sales);

        client.set_protocol_fee(&admin, &10_000); // 100%
        client.set_min_topup(&admin, &1);

        let subscriber = Address::generate(&env);
        let stellar = soroban_sdk::token::StellarAssetClient::new(&env, &token);
        stellar.mint(&subscriber, &10_000_000);

        let sub_id = client
            .create_subscription_with_token(
                &subscriber,
                &merchant,
                &token,
                &1_000_000,
                &86400,
                &false,
                &None::<i128>,
                &None::<u64>,
                &None::<u32>,
                &Some(sales.clone()),
            )
            .unwrap();

        client.deposit_funds(&sub_id, &5_000_000, &None);
        client.charge_subscription(&sub_id, &None);

        let balance = client.get_sub_account_balance(&merchant, &sales);
        assert_eq!(
            balance, 0,
            "Sub-account receives nothing when fee is 100%"
        );
    }
}
