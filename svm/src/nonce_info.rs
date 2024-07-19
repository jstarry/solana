use solana_sdk::{
    account::AccountSharedData,
    account_utils::StateMut,
    nonce::state::{DurableNonce, State as NonceState, Versions as NonceVersions},
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

    pub fn get_advanced_account(
        &mut self,
        durable_nonce: DurableNonce,
        lamports_per_signature: u64,
    ) -> (&Pubkey, &AccountSharedData) {
        // Advance the stored blockhash to prevent fee theft by someone
        // replaying nonce transactions that have failed with an
        // `InstructionError`.
        //
        // Since we know we are dealing with a valid nonce account,
        // unwrap is safe here
        let nonce_versions = StateMut::<NonceVersions>::state(&self.account).unwrap();
        if let NonceState::Initialized(ref data) = nonce_versions.state() {
            let nonce_state =
                NonceState::new_initialized(&data.authority, durable_nonce, lamports_per_signature);
            let nonce_versions = NonceVersions::new(nonce_state);
            self.account.set_state(&nonce_versions).unwrap();
        }

        (&self.address, &self.account)
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
            hash::Hash,
            nonce::state::{
                Data as NonceData, DurableNonce, State as NonceState, Versions as NonceVersions,
            },
            system_program,
        },
    };

    #[test]
    fn test_nonce_info() {
        let nonce_address = Pubkey::new_unique();
        let durable_nonce = DurableNonce::from_blockhash(&Hash::new_unique());
        let lamports_per_signature = 42;
        let nonce_account = AccountSharedData::new_data(
            43,
            &NonceVersions::new(NonceState::Initialized(NonceData::new(
                Pubkey::default(),
                durable_nonce,
                lamports_per_signature,
            ))),
            &system_program::id(),
        )
        .unwrap();

        // NoncePartial create + NonceInfo impl
        let partial = NoncePartial::new(nonce_address, nonce_account.clone());
        assert_eq!(*partial.address(), nonce_address);
        assert_eq!(*partial.account(), nonce_account);
        assert_eq!(
            partial.lamports_per_signature(),
            Some(lamports_per_signature)
        );
        assert_eq!(partial.fee_payer_account(), None);
    }
}
