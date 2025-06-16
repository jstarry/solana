use {
    solana_hash::Hash,
    solana_message::{
        legacy,
        v0::{self, LoadedMessage},
        AccountKeys, AddressLoader, LegacyMessage, SanitizedMessage, SanitizedVersionedMessage,
        VersionedMessage,
    },
    solana_pubkey::Pubkey,
    solana_signature::Signature,
    solana_svm_transaction::{
        instruction::SVMInstruction, message_address_table_lookup::SVMMessageAddressTableLookup,
        svm_message::SVMMessage,
    },
    solana_transaction::{
        sanitized::MessageHash,
        simple_vote_transaction_checker,
        versioned::{sanitized::SanitizedVersionedTransaction, VersionedTransaction},
    },
    solana_transaction_error::TransactionResult,
    std::collections::HashSet,
};

#[cfg_attr(feature = "dev-context-only-utils", derive(Clone))]
#[derive(Debug)]
pub struct ResolvedTransaction {
    pub message: SanitizedMessage,
    pub message_hash: Hash,
    pub is_simple_vote_tx: bool,
    pub signatures: Vec<Signature>,
}

impl ResolvedTransaction {
    pub fn try_new(
        tx: SanitizedVersionedTransaction,
        message_hash: Hash,
        is_simple_vote_tx: bool,
        address_loader: impl AddressLoader,
        reserved_account_keys: &HashSet<Pubkey>,
    ) -> TransactionResult<Self> {
        let (signatures, SanitizedVersionedMessage { message }) = tx.destruct();
        let message = match message {
            VersionedMessage::Legacy(message) => {
                SanitizedMessage::Legacy(LegacyMessage::new(message, reserved_account_keys))
            }
            VersionedMessage::V0(message) => {
                let loaded_addresses =
                    address_loader.load_addresses(&message.address_table_lookups)?;
                SanitizedMessage::V0(LoadedMessage::new(
                    message,
                    loaded_addresses,
                    reserved_account_keys,
                ))
            }
        };

        Ok(Self {
            message,
            message_hash,
            is_simple_vote_tx,
            signatures,
        })
    }

    /// Create a sanitized transaction from an un-sanitized versioned
    /// transaction.  If the input transaction uses address tables, attempt to
    /// lookup the address for each table index.
    pub fn try_create(
        tx: VersionedTransaction,
        message_hash: impl Into<MessageHash>,
        is_simple_vote_tx: Option<bool>,
        address_loader: impl AddressLoader,
        reserved_account_keys: &HashSet<Pubkey>,
    ) -> TransactionResult<Self> {
        let sanitized_versioned_tx = SanitizedVersionedTransaction::try_from(tx)?;
        let is_simple_vote_tx = is_simple_vote_tx.unwrap_or_else(|| {
            simple_vote_transaction_checker::is_simple_vote_transaction(&sanitized_versioned_tx)
        });
        let message_hash = match message_hash.into() {
            MessageHash::Compute => sanitized_versioned_tx.get_message().message.hash(),
            MessageHash::Precomputed(hash) => hash,
        };
        Self::try_new(
            sanitized_versioned_tx,
            message_hash,
            is_simple_vote_tx,
            address_loader,
            reserved_account_keys,
        )
    }

    /// Return the first signature for this transaction.
    ///
    /// Notes:
    ///
    /// Sanitized transactions must have at least one signature because the
    /// number of signatures must be greater than or equal to the message header
    /// value `num_required_signatures` which must be greater than 0 itself.
    pub fn signature(&self) -> &Signature {
        &self.signatures[0]
    }

    /// Return the list of signatures for this transaction
    pub fn signatures(&self) -> &[Signature] {
        &self.signatures
    }

    /// Return the signed message
    pub fn message(&self) -> &SanitizedMessage {
        &self.message
    }

    /// Return the hash of the signed message
    pub fn message_hash(&self) -> &Hash {
        &self.message_hash
    }

    /// Returns true if this transaction is a simple vote
    pub fn is_simple_vote_transaction(&self) -> bool {
        self.is_simple_vote_tx
    }

    /// Convert this sanitized transaction into a versioned transaction for
    /// recording in the ledger.
    pub fn to_versioned_transaction(&self) -> VersionedTransaction {
        let signatures = self.signatures.clone();
        match &self.message {
            SanitizedMessage::V0(sanitized_msg) => VersionedTransaction {
                signatures,
                message: VersionedMessage::V0(v0::Message::clone(&sanitized_msg.message)),
            },
            SanitizedMessage::Legacy(legacy_message) => VersionedTransaction {
                signatures,
                message: VersionedMessage::Legacy(legacy::Message::clone(&legacy_message.message)),
            },
        }
    }
}

impl SVMMessage for ResolvedTransaction {
    fn num_transaction_signatures(&self) -> u64 {
        self.message.num_transaction_signatures()
    }

    fn num_ed25519_signatures(&self) -> u64 {
        self.message.num_ed25519_signatures()
    }

    fn num_secp256k1_signatures(&self) -> u64 {
        self.message.num_secp256k1_signatures()
    }

    fn num_secp256r1_signatures(&self) -> u64 {
        self.message.num_secp256r1_signatures()
    }

    fn num_write_locks(&self) -> u64 {
        self.message.num_write_locks()
    }

    fn recent_blockhash(&self) -> &Hash {
        self.message.recent_blockhash()
    }

    fn num_instructions(&self) -> usize {
        self.message.num_instructions()
    }

    fn instructions_iter(&self) -> impl Iterator<Item = SVMInstruction> {
        self.message.instructions_iter()
    }

    fn program_instructions_iter(&self) -> impl Iterator<Item = (&Pubkey, SVMInstruction)> + Clone {
        SVMMessage::program_instructions_iter(&self.message)
    }

    fn static_account_keys(&self) -> &[Pubkey] {
        self.message.static_account_keys()
    }

    fn account_keys(&self) -> AccountKeys {
        self.message.account_keys()
    }

    fn fee_payer(&self) -> &Pubkey {
        self.message.fee_payer()
    }

    fn is_writable(&self, index: usize) -> bool {
        self.message.is_writable(index)
    }

    fn is_signer(&self, index: usize) -> bool {
        self.message.is_signer(index)
    }

    fn is_invoked(&self, key_index: usize) -> bool {
        self.message.is_invoked(key_index)
    }

    fn num_lookup_tables(&self) -> usize {
        self.message.num_lookup_tables()
    }

    fn message_address_table_lookups(&self) -> impl Iterator<Item = SVMMessageAddressTableLookup> {
        SVMMessage::message_address_table_lookups(&self.message)
    }
}
