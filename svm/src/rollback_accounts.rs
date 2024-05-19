use {
    crate::nonce_info::{NonceInfo, NoncePartial},
    solana_sdk::{
        account::{AccountSharedData, ReadableAccount, WritableAccount},
        nonce::state::DurableNonce,
        pubkey::Pubkey,
        rent_debits::RentDebits,
    },
};

/// Captured account state used to rollback account state for nonce and fee
/// payer accounts after a failed executed transaction.
#[derive(PartialEq, Eq, Debug, Clone)]
pub enum RollbackAccounts {
    FeePayerOnly {
        fee_payer_account: AccountSharedData,
    },
    SameNonceAndFeePayer {
        nonce: NoncePartial,
    },
    SeparateNonceAndFeePayer {
        nonce: NoncePartial,
        fee_payer_account: AccountSharedData,
    },
}

impl RollbackAccounts {
    pub fn new(
        nonce: Option<&NoncePartial>,
        fee_payer_address: Pubkey,
        mut fee_payer_account: AccountSharedData,
        rent_debits: &RentDebits,
    ) -> Self {
        let rent_debit = rent_debits.get_account_rent_debit(&fee_payer_address);
        fee_payer_account.set_lamports(fee_payer_account.lamports().saturating_add(rent_debit));

        if let Some(nonce) = nonce {
            if &fee_payer_address == nonce.address() {
                RollbackAccounts::SameNonceAndFeePayer {
                    nonce: NoncePartial::new(fee_payer_address, fee_payer_account),
                }
            } else {
                RollbackAccounts::SeparateNonceAndFeePayer {
                    nonce: nonce.clone(),
                    fee_payer_account,
                }
            }
        } else {
            RollbackAccounts::FeePayerOnly { fee_payer_account }
        }
    }

    pub fn nonce(&self) -> Option<&NoncePartial> {
        match self {
            Self::FeePayerOnly { .. } => None,
            Self::SameNonceAndFeePayer { nonce } | Self::SeparateNonceAndFeePayer { nonce, .. } => {
                Some(nonce)
            }
        }
    }

    pub fn rollback_account_for_failed_tx(
        &self,
        address: &Pubkey,
        account: &mut AccountSharedData,
        is_fee_payer: bool,
        &durable_nonce: &DurableNonce,
        lamports_per_signature: u64,
    ) -> bool {
        let rollback_account = match self {
            Self::FeePayerOnly { fee_payer_account } => {
                if is_fee_payer {
                    Some(fee_payer_account.clone())
                } else {
                    None
                }
            }
            Self::SameNonceAndFeePayer { nonce } => {
                if is_fee_payer {
                    Some(nonce.advance_nonce_account(durable_nonce, lamports_per_signature))
                } else {
                    None
                }
            }
            Self::SeparateNonceAndFeePayer {
                nonce,
                fee_payer_account,
            } => {
                if is_fee_payer {
                    Some(fee_payer_account.clone())
                } else if address == nonce.address() {
                    Some(nonce.advance_nonce_account(durable_nonce, lamports_per_signature))
                } else {
                    None
                }
            }
        };

        if let Some(rollback_account) = rollback_account {
            *account = rollback_account;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        solana_sdk::{
            account_utils::StateMut,
            hash::Hash,
            nonce::state::{
                Data as NonceData, DurableNonce, State as NonceState, Versions as NonceVersions,
            },
            system_program,
        },
    };

    fn create_accounts_rollback_account_for_failed_tx() -> (
        Pubkey,
        AccountSharedData,
        AccountSharedData,
        DurableNonce,
        u64,
    ) {
        let data = NonceVersions::new(NonceState::Initialized(NonceData::default()));
        let account = AccountSharedData::new_data(42, &data, &system_program::id()).unwrap();
        let mut pre_account = account.clone();
        pre_account.set_lamports(43);
        let durable_nonce = DurableNonce::from_blockhash(&Hash::new(&[1u8; 32]));
        (Pubkey::default(), pre_account, account, durable_nonce, 1234)
    }

    fn run_rollback_account_for_failed_tx_test(
        account_address: &Pubkey,
        account: &mut AccountSharedData,
        is_fee_payer: bool,
        rollback_accounts: &RollbackAccounts,
        durable_nonce: &DurableNonce,
        lamports_per_signature: u64,
        expect_account: &AccountSharedData,
    ) -> bool {
        // If expect_account is a nonce account, make sure the rollback nonce account
        // isn't equal to the expect account yet (because it still needs to be advanced)
        match rollback_accounts {
            RollbackAccounts::SeparateNonceAndFeePayer { nonce, .. }
            | RollbackAccounts::SameNonceAndFeePayer { nonce } => {
                if nonce.address() == account_address {
                    assert_ne!(expect_account, nonce.account());
                }
            }
            _ => {}
        }

        rollback_accounts.rollback_account_for_failed_tx(
            account_address,
            account,
            is_fee_payer,
            durable_nonce,
            lamports_per_signature,
        );
        assert_eq!(expect_account, account);
        expect_account == account
    }

    #[test]
    fn test_rollback_account_for_failed_tx_expected() {
        let (pre_account_address, pre_account, mut post_account, blockhash, lamports_per_signature) =
            create_accounts_rollback_account_for_failed_tx();
        let post_account_address = pre_account_address;
        let nonce = NoncePartial::new(pre_account_address, pre_account.clone());

        let mut expect_account = pre_account;
        expect_account
            .set_state(&NonceVersions::new(NonceState::Initialized(
                NonceData::new(Pubkey::default(), blockhash, lamports_per_signature),
            )))
            .unwrap();

        assert!(run_rollback_account_for_failed_tx_test(
            &post_account_address,
            &mut post_account,
            false,
            &RollbackAccounts::SeparateNonceAndFeePayer {
                nonce,
                fee_payer_account: AccountSharedData::default()
            },
            &blockhash,
            lamports_per_signature,
            &expect_account,
        ));
    }

    #[test]
    fn test_rollback_account_for_failed_tx_not_nonce_address() {
        let (pre_account_address, pre_account, mut post_account, blockhash, lamports_per_signature) =
            create_accounts_rollback_account_for_failed_tx();

        let nonce = NoncePartial::new(pre_account_address, pre_account);

        let expect_account = post_account.clone();
        // Wrong key
        assert!(run_rollback_account_for_failed_tx_test(
            &Pubkey::from([1u8; 32]),
            &mut post_account,
            false,
            &RollbackAccounts::SeparateNonceAndFeePayer {
                nonce,
                fee_payer_account: AccountSharedData::default()
            },
            &blockhash,
            lamports_per_signature,
            &expect_account,
        ));
    }

    #[test]
    fn test_rollback_nonce_fee_payer() {
        let nonce_account = AccountSharedData::new_data(1, &(), &system_program::id()).unwrap();
        let pre_fee_payer_account =
            AccountSharedData::new_data(84, &[1, 2, 3, 4], &system_program::id()).unwrap();
        let post_fee_payer_account =
            AccountSharedData::new_data(42, &(), &system_program::id()).unwrap();
        let nonce = NoncePartial::new(Pubkey::new_unique(), nonce_account);
        let rollback_accounts = RollbackAccounts::SeparateNonceAndFeePayer {
            nonce,
            fee_payer_account: post_fee_payer_account.clone(),
        };

        assert!(run_rollback_account_for_failed_tx_test(
            &Pubkey::new_unique(),
            &mut pre_fee_payer_account.clone(),
            false,
            &rollback_accounts,
            &DurableNonce::default(),
            1,
            &pre_fee_payer_account,
        ));

        assert!(run_rollback_account_for_failed_tx_test(
            &Pubkey::new_unique(),
            &mut pre_fee_payer_account.clone(),
            true,
            &rollback_accounts,
            &DurableNonce::default(),
            1,
            &post_fee_payer_account,
        ));
    }
}
