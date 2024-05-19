use solana_sdk::{
    account::AccountSharedData,
    account_utils::StateMut,
    nonce::state::{DurableNonce, State, Versions},
    nonce_account,
    pubkey::Pubkey,
};

pub trait NonceInfo {
    fn address(&self) -> &Pubkey;
    fn account(&self) -> &AccountSharedData;
    fn lamports_per_signature(&self) -> Option<u64>;
    fn fee_payer_account(&self) -> Option<&AccountSharedData>;
}

/// Holds limited nonce info available during transaction checks
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NoncePartial {
    address: Pubkey,
    account: AccountSharedData,
}

impl NoncePartial {
    pub fn new(address: Pubkey, account: AccountSharedData) -> Self {
        Self { address, account }
    }

    /// Advance the stored durable nonce for the captured nonce account
    /// and update lamports per signature.
    pub fn advance_nonce_account(
        &self,
        durable_nonce: DurableNonce,
        lamports_per_signature: u64,
    ) -> AccountSharedData {
        // The transaction failed which would normally drop the account
        // processing changes, since this account is now being included
        // in the accounts written back to the db, roll it back to
        // pre-processing state.
        let mut account = self.account.clone();

        // Advance the stored blockhash to prevent fee theft by someone
        // replaying nonce transactions that have failed with an
        // `InstructionError`.
        //
        // Since we know we are dealing with a valid nonce account,
        // unwrap is safe here
        let nonce_versions = StateMut::<Versions>::state(&account).unwrap();
        if let State::Initialized(ref data) = nonce_versions.state() {
            let nonce_state =
                State::new_initialized(&data.authority, durable_nonce, lamports_per_signature);
            let nonce_versions = Versions::new(nonce_state);
            account.set_state(&nonce_versions).unwrap();
        }

        // Return nonce account with an advanced nonce blockhash
        account
    }
}

impl NonceInfo for NoncePartial {
    fn address(&self) -> &Pubkey {
        &self.address
    }
    fn account(&self) -> &AccountSharedData {
        &self.account
    }
    fn lamports_per_signature(&self) -> Option<u64> {
        nonce_account::lamports_per_signature_of(&self.account)
    }
    fn fee_payer_account(&self) -> Option<&AccountSharedData> {
        None
    }
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        solana_sdk::{
            account::{ReadableAccount, WritableAccount},
            hash::Hash,
            nonce::{self, state::DurableNonce},
            rent_debits::RentDebits,
            signature::{keypair_from_seed, Signer},
            system_program,
        },
    };

    #[test]
    fn test_nonce_info() {
        let lamports_per_signature = 42;

        let nonce_authority = keypair_from_seed(&[0; 32]).unwrap();
        let nonce_address = nonce_authority.pubkey();
        let from = keypair_from_seed(&[1; 32]).unwrap();
        let from_address = from.pubkey();

        let durable_nonce = DurableNonce::from_blockhash(&Hash::new_unique());
        let nonce_account = AccountSharedData::new_data(
            43,
            &nonce::state::Versions::new(nonce::State::Initialized(nonce::state::Data::new(
                Pubkey::default(),
                durable_nonce,
                lamports_per_signature,
            ))),
            &system_program::id(),
        )
        .unwrap();
        let from_account = AccountSharedData::new(44, 0, &Pubkey::default());

        const TEST_RENT_DEBIT: u64 = 1;
        let rent_collected_nonce_account = {
            let mut account = nonce_account.clone();
            account.set_lamports(nonce_account.lamports() - TEST_RENT_DEBIT);
            account
        };
        let rent_collected_from_account = {
            let mut account = from_account.clone();
            account.set_lamports(from_account.lamports() - TEST_RENT_DEBIT);
            account
        };

        // NoncePartial create + NonceInfo impl
        let partial = NoncePartial::new(nonce_address, rent_collected_nonce_account.clone());
        assert_eq!(*partial.address(), nonce_address);
        assert_eq!(*partial.account(), rent_collected_nonce_account);
        assert_eq!(
            partial.lamports_per_signature(),
            Some(lamports_per_signature)
        );
        assert_eq!(partial.fee_payer_account(), None);

        // Add rent debits to ensure the rollback captures accounts without rent fees
        let mut rent_debits = RentDebits::default();
        rent_debits.insert(
            &from_address,
            TEST_RENT_DEBIT,
            rent_collected_from_account.lamports(),
        );
        rent_debits.insert(
            &nonce_address,
            TEST_RENT_DEBIT,
            rent_collected_nonce_account.lamports(),
        );
    }
}
