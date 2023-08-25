pub mod wallet_actions;

use std::cmp;
use std::collections::HashMap;
use std::fmt::Debug;
use std::path::Path;
use zcash_client_backend::data_api::wallet::ANCHOR_OFFSET;
use zcash_client_backend::data_api::{PrunedBlock, ReceivedTransaction, SentTransaction};
use zcash_client_backend::wallet::{AccountId, SpendableNote};
use zcash_primitives::block::BlockHash;
use zcash_primitives::consensus::BlockHeight;
use zcash_primitives::memo::Memo;
use zcash_primitives::merkle_tree::{CommitmentTree, IncrementalWitness};
use zcash_primitives::sapling::{Node, Nullifier, PaymentAddress};
use zcash_primitives::transaction::components::Amount;
use zcash_primitives::transaction::TxId;
use zcash_primitives::zip32::ExtendedFullViewingKey;

#[async_trait::async_trait]
pub trait WalletRead: Send + Sync + 'static {
    type Error;
    type NoteRef: Copy + Debug;
    type TxRef: Copy + Debug;

    /// Returns the minimum and maximum block heights for stored blocks.
    ///
    /// This will return `Ok(None)` if no block data is present in the database.
    async fn block_height_extrema(&self)
        -> Result<Option<(BlockHeight, BlockHeight)>, Self::Error>;

    /// Returns the default target height (for the block in which a new
    /// transaction would be mined) and anchor height (to use for a new
    /// transaction), given the range of block heights that the backend
    /// knows about.
    ///
    /// This will return `Ok(None)` if no block data is present in the database.
    async fn get_target_and_anchor_heights(
        &self,
    ) -> Result<Option<(BlockHeight, BlockHeight)>, Self::Error> {
        self.block_height_extrema().await.map(|heights| {
            heights.map(|(min_height, max_height)| {
                let target_height = max_height + 1;

                // Select an anchor ANCHOR_OFFSET back from the target block,
                // unless that would be before the earliest block we have.
                let anchor_height = BlockHeight::from(cmp::max(
                    u32::from(target_height).saturating_sub(ANCHOR_OFFSET),
                    u32::from(min_height),
                ));

                (target_height, anchor_height)
            })
        })
    }

    /// Returns the block hash for the block at the given height, if the
    /// associated block data is available. Returns `Ok(None)` if the hash
    /// is not found in the database.
    async fn get_block_hash(
        &self,
        block_height: BlockHeight,
    ) -> Result<Option<BlockHash>, Self::Error>;

    /// Returns the block hash for the block at the maximum height known
    /// in stored data.
    ///
    /// This will return `Ok(None)` if no block data is present in the database.
    async fn get_max_height_hash(&self) -> Result<Option<(BlockHeight, BlockHash)>, Self::Error> {
        let extrema = self.block_height_extrema().await?;
        let res = if let Some((_, max_height)) = extrema {
            self.get_block_hash(max_height)
                .await
                .map(|hash_opt| hash_opt.map(move |hash| (max_height, hash)))?
        } else {
            None
        };

        Ok(res)
    }

    /// Returns the block height in which the specified transaction was mined,
    /// or `Ok(None)` if the transaction is not mined in the main chain.
    async fn get_tx_height(&self, txid: TxId) -> Result<Option<BlockHeight>, Self::Error>;

    /// Returns the payment address for the specified account, if the account
    /// identifier specified refers to a valid account for this wallet.
    ///
    /// This will return `Ok(None)` if the account identifier does not correspond
    /// to a known account.
    async fn get_address(&self, account: AccountId) -> Result<Option<PaymentAddress>, Self::Error>;

    /// Returns all extended full viewing keys known about by this wallet.
    async fn get_extended_full_viewing_keys(
        &self,
    ) -> Result<HashMap<AccountId, ExtendedFullViewingKey>, Self::Error>;

    /// Checks whether the specified extended full viewing key is
    /// associated with the account.
    async fn is_valid_account_extfvk(
        &self,
        account: AccountId,
        extfvk: &ExtendedFullViewingKey,
    ) -> Result<bool, Self::Error>;

    /// Returns the wallet balance for an account as of the specified block
    /// height.
    ///
    /// This may be used to obtain a balance that ignores notes that have been
    /// received so recently that they are not yet deemed spendable.
    async fn get_balance_at(
        &self,
        account: AccountId,
        anchor_height: BlockHeight,
    ) -> Result<Amount, Self::Error>;

    /// Returns the memo for a note.
    ///
    /// Implementations of this method must return an error if the note identifier
    /// does not appear in the backing data store.
    async fn get_memo(&self, id_note: Self::NoteRef) -> Result<Memo, Self::Error>;

    /// Returns the note commitment tree at the specified block height.
    async fn get_commitment_tree(
        &self,
        block_height: BlockHeight,
    ) -> Result<Option<CommitmentTree<Node>>, Self::Error>;

    /// Returns the incremental witnesses as of the specified block height.
    #[allow(clippy::type_complexity)]
    async fn get_witnesses(
        &self,
        block_height: BlockHeight,
    ) -> Result<Vec<(Self::NoteRef, IncrementalWitness<Node>)>, Self::Error>;

    /// Returns the unspent nullifiers, along with the account identifiers
    /// with which they are associated.
    async fn get_nullifiers(&self) -> Result<Vec<(AccountId, Nullifier)>, Self::Error>;

    /// Return all spendable notes.
    async fn get_spendable_notes(
        &self,
        account: AccountId,
        anchor_height: BlockHeight,
    ) -> Result<Vec<SpendableNote>, Self::Error>;

    /// Returns a list of spendable notes sufficient to cover the specified
    /// target value, if possible.
    async fn select_spendable_notes(
        &self,
        account: AccountId,
        target_value: Amount,
        anchor_height: BlockHeight,
    ) -> Result<Vec<SpendableNote>, Self::Error>;
}

/// This trait encapsulates the write capabilities required to update stored
/// wallet data.
#[async_trait::async_trait]
pub trait WalletWrite: WalletRead {
    #[allow(clippy::type_complexity)]
    async fn advance_by_block(
        &mut self,
        block: &PrunedBlock,
        updated_witnesses: &[(Self::NoteRef, IncrementalWitness<Node>)],
    ) -> Result<Vec<(Self::NoteRef, IncrementalWitness<Node>)>, Self::Error>;

    async fn store_received_tx(
        &mut self,
        received_tx: &ReceivedTransaction,
    ) -> Result<Self::TxRef, Self::Error>;

    async fn store_sent_tx(
        &mut self,
        sent_tx: &SentTransaction,
    ) -> Result<Self::TxRef, Self::Error>;

    /// Rewinds the wallet database to the specified height.
    ///
    /// This method assumes that the state of the underlying data store is
    /// consistent up to a particular block height. Since it is possible that
    /// a chain reorg might invalidate some stored state, this method must be
    /// implemented in order to allow users of this API to "reset" the data store
    /// to correctly represent chainstate as of a specified block height.
    ///
    /// After calling this method, the block at the given height will be the
    /// most recent block and all other operations will treat this block
    /// as the chain tip for balance determination purposes.
    ///
    /// There may be restrictions on how far it is possible to rewind.
    async fn rewind_to_height(&mut self, block_height: BlockHeight) -> Result<(), Self::Error>;
}

use crate::error::SqliteClientError;
use crate::{wallet, NoteId, WalletDb};
use rusqlite::{Connection, OptionalExtension, Statement, ToSql};
use std::sync::{Arc, Mutex};
use zcash_client_backend::encoding::{
    decode_extended_full_viewing_key, decode_payment_address, encode_extended_full_viewing_key,
};
use zcash_primitives::consensus;

/// A wrapper for the SQLite connection to the wallet database.
#[derive(Clone)]
pub struct WalletDbAsync<P> {
    inner: Arc<Mutex<WalletDb<P>>>,
}

impl<P: consensus::Parameters> WalletDbAsync<P> {
    pub fn inner(&self) -> Arc<Mutex<WalletDb<P>>> {
        self.inner.clone()
    }

    /// Construct a connection to the wallet database stored at the specified path.
    pub fn for_path<F: AsRef<Path>>(path: F, params: P) -> Result<Self, rusqlite::Error> {
        let db = Connection::open(path).map(move |conn| WalletDb { conn, params })?;
        Ok(Self {
            inner: Arc::new(Mutex::new(db)),
        })
    }

    /// Given a wallet database connection, obtain a handle for the write operations
    /// for that database. This operation may eagerly initialize and cache sqlite
    /// prepared statements that are used in write operations.
    pub fn get_update_ops(&self) -> Result<DataConnStmtCacheAsync<P>, SqliteClientError> {
        Ok(DataConnStmtCacheAsync {
            wallet_db: self.clone(),
        })
    }
}

#[async_trait::async_trait]
impl<P: consensus::Parameters + Send + Sync + 'static> WalletRead for WalletDbAsync<P> {
    type Error = SqliteClientError;
    type NoteRef = NoteId;
    type TxRef = i64;

    async fn block_height_extrema(
        &self,
    ) -> Result<Option<(BlockHeight, BlockHeight)>, Self::Error> {
        let db = self.inner.lock().unwrap();
        wallet::block_height_extrema(&db).map_err(SqliteClientError::from)
    }

    async fn get_block_hash(
        &self,
        block_height: BlockHeight,
    ) -> Result<Option<BlockHash>, Self::Error> {
        let db = self.inner.lock().unwrap();
        wallet::get_block_hash(&db, block_height).map_err(SqliteClientError::from)
    }

    async fn get_tx_height(&self, txid: TxId) -> Result<Option<BlockHeight>, Self::Error> {
        let db = self.inner.lock().unwrap();
        wallet::get_tx_height(&db, txid).map_err(SqliteClientError::from)
    }

    async fn get_address(&self, account: AccountId) -> Result<Option<PaymentAddress>, Self::Error> {
        let db = self.inner.lock().unwrap();
        wallet::get_address(&db, account).map_err(SqliteClientError::from)
    }

    async fn get_extended_full_viewing_keys(
        &self,
    ) -> Result<HashMap<AccountId, ExtendedFullViewingKey>, Self::Error> {
        let db = self.inner.lock().unwrap();
        wallet::get_extended_full_viewing_keys(&db)
    }

    async fn is_valid_account_extfvk(
        &self,
        account: AccountId,
        extfvk: &ExtendedFullViewingKey,
    ) -> Result<bool, Self::Error> {
        let db = self.inner.lock().unwrap();
        wallet::is_valid_account_extfvk(&db, account, extfvk)
    }

    async fn get_balance_at(
        &self,
        account: AccountId,
        anchor_height: BlockHeight,
    ) -> Result<Amount, Self::Error> {
        let db = self.inner.lock().unwrap();
        wallet::get_balance_at(&db, account, anchor_height)
    }

    async fn get_memo(&self, id_note: Self::NoteRef) -> Result<Memo, Self::Error> {
        let db = self.inner.lock().unwrap();
        match id_note {
            NoteId::SentNoteId(id_note) => wallet::get_sent_memo(&db, id_note),
            NoteId::ReceivedNoteId(id_note) => wallet::get_received_memo(&db, id_note),
        }
    }

    async fn get_commitment_tree(
        &self,
        block_height: BlockHeight,
    ) -> Result<Option<CommitmentTree<Node>>, Self::Error> {
        let db = self.inner.lock().unwrap();
        wallet::get_commitment_tree(&db, block_height)
    }

    #[allow(clippy::type_complexity)]
    async fn get_witnesses(
        &self,
        block_height: BlockHeight,
    ) -> Result<Vec<(Self::NoteRef, IncrementalWitness<Node>)>, Self::Error> {
        let db = self.inner.lock().unwrap();
        wallet::get_witnesses(&db, block_height)
    }

    async fn get_nullifiers(&self) -> Result<Vec<(AccountId, Nullifier)>, Self::Error> {
        let db = self.inner.lock().unwrap();
        wallet::get_nullifiers(&db)
    }

    async fn get_spendable_notes(
        &self,
        account: AccountId,
        anchor_height: BlockHeight,
    ) -> Result<Vec<SpendableNote>, Self::Error> {
        let db = self.inner.lock().unwrap();
        wallet::transact::get_spendable_notes(&db, account, anchor_height)
    }

    async fn select_spendable_notes(
        &self,
        account: AccountId,
        target_value: Amount,
        anchor_height: BlockHeight,
    ) -> Result<Vec<SpendableNote>, Self::Error> {
        let db = self.inner.lock().unwrap();
        wallet::transact::select_spendable_notes(&db, account, target_value, anchor_height)
    }
}

pub struct DataConnStmtCacheAsync<P> {
    wallet_db: WalletDbAsync<P>,
}

impl<P: consensus::Parameters> DataConnStmtCacheAsync<P> {
    fn transactionally<F, A>(&mut self, f: F) -> Result<A, SqliteClientError>
    where
        F: FnOnce(&mut Self) -> Result<A, SqliteClientError>,
    {
        self.wallet_db
            .inner
            .lock()
            .unwrap()
            .conn
            .execute("BEGIN IMMEDIATE", [])?;
        match f(self) {
            Ok(result) => {
                self.wallet_db
                    .inner
                    .lock()
                    .unwrap()
                    .conn
                    .execute("COMMIT", [])?;
                Ok(result)
            }
            Err(error) => {
                match self.wallet_db.inner.lock().unwrap().conn.execute("ROLLBACK", []) {
                    Ok(_) => Err(error),
                    Err(e) =>
                    // Panicking here is probably the right thing to do, because it
                    // means the database is corrupt.
                        panic!(
                            "Rollback failed with error {} while attempting to recover from error {}; database is likely corrupt.",
                            e,
                            error
                        )
                }
            }
        }
    }
}

#[async_trait::async_trait]
impl<P: consensus::Parameters + Send + Sync + 'static> WalletRead for DataConnStmtCacheAsync<P> {
    type Error = SqliteClientError;
    type NoteRef = NoteId;
    type TxRef = i64;

    async fn block_height_extrema(
        &self,
    ) -> Result<Option<(BlockHeight, BlockHeight)>, Self::Error> {
        self.block_height_extrema().await
    }

    async fn get_block_hash(
        &self,
        block_height: BlockHeight,
    ) -> Result<Option<BlockHash>, Self::Error> {
        self.get_block_hash(block_height).await
    }

    async fn get_tx_height(&self, txid: TxId) -> Result<Option<BlockHeight>, Self::Error> {
        self.get_tx_height(txid).await
    }

    async fn get_address(&self, account: AccountId) -> Result<Option<PaymentAddress>, Self::Error> {
        self.get_address(account).await
    }

    async fn get_extended_full_viewing_keys(
        &self,
    ) -> Result<HashMap<AccountId, ExtendedFullViewingKey>, Self::Error> {
        self.get_extended_full_viewing_keys().await
    }

    async fn is_valid_account_extfvk(
        &self,
        account: AccountId,
        extfvk: &ExtendedFullViewingKey,
    ) -> Result<bool, Self::Error> {
        self.is_valid_account_extfvk(account, extfvk).await
    }

    async fn get_balance_at(
        &self,
        account: AccountId,
        anchor_height: BlockHeight,
    ) -> Result<Amount, Self::Error> {
        self.get_balance_at(account, anchor_height).await
    }

    async fn get_memo(&self, id_note: Self::NoteRef) -> Result<Memo, Self::Error> {
        self.get_memo(id_note).await
    }

    async fn get_commitment_tree(
        &self,
        block_height: BlockHeight,
    ) -> Result<Option<CommitmentTree<Node>>, Self::Error> {
        self.get_commitment_tree(block_height).await
    }

    #[allow(clippy::type_complexity)]
    async fn get_witnesses(
        &self,
        block_height: BlockHeight,
    ) -> Result<Vec<(Self::NoteRef, IncrementalWitness<Node>)>, Self::Error> {
        self.get_witnesses(block_height).await
    }

    async fn get_nullifiers(&self) -> Result<Vec<(AccountId, Nullifier)>, Self::Error> {
        self.get_nullifiers().await
    }

    async fn get_spendable_notes(
        &self,
        account: AccountId,
        anchor_height: BlockHeight,
    ) -> Result<Vec<SpendableNote>, Self::Error> {
        self.get_spendable_notes(account, anchor_height).await
    }

    async fn select_spendable_notes(
        &self,
        account: AccountId,
        target_value: Amount,
        anchor_height: BlockHeight,
    ) -> Result<Vec<SpendableNote>, Self::Error> {
        self.select_spendable_notes(account, target_value, anchor_height)
            .await
    }
}

#[async_trait::async_trait]
impl<P: consensus::Parameters + Send + Sync + 'static> WalletWrite for DataConnStmtCacheAsync<P> {
    #[allow(clippy::type_complexity)]
    async fn advance_by_block(
        &mut self,
        block: &PrunedBlock,
        updated_witnesses: &[(Self::NoteRef, IncrementalWitness<Node>)],
    ) -> Result<Vec<(Self::NoteRef, IncrementalWitness<Node>)>, Self::Error> {
        // database updates for each block are transactional
        self.transactionally(|up| {
            let db = up.wallet_db.inner.lock().unwrap();
            // Insert the block into the database.
            wallet_actions::insert_block(
                &db,
                block.block_height,
                block.block_hash,
                block.block_time,
                &block.commitment_tree,
            )?;

            let mut new_witnesses = vec![];
            for tx in block.transactions {
                let tx_row = wallet_actions::put_tx_meta(&db, &tx, block.block_height)?;

                // Mark notes as spent and remove them from the scanning cache
                for spend in &tx.shielded_spends {
                    wallet_actions::mark_spent(&db, tx_row, &spend.nf)?;
                }

                for output in &tx.shielded_outputs {
                    let received_note_id = wallet_actions::put_received_note(&db, output, tx_row)?;

                    // Save witness for note.
                    new_witnesses.push((received_note_id, output.witness.clone()));
                }
            }

            // Insert current new_witnesses into the database.
            for (received_note_id, witness) in updated_witnesses.iter().chain(new_witnesses.iter())
            {
                if let NoteId::ReceivedNoteId(rnid) = *received_note_id {
                    wallet_actions::insert_witness(&db, rnid, witness, block.block_height)?;
                } else {
                    return Err(SqliteClientError::InvalidNoteId);
                }
            }

            // Prune the stored witnesses (we only expect rollbacks of at most 100 blocks).
            let below_height = if block.block_height < BlockHeight::from(100) {
                BlockHeight::from(0)
            } else {
                block.block_height - 100
            };
            wallet_actions::prune_witnesses(&db, below_height)?;

            // Update now-expired transactions that didn't get mined.
            wallet_actions::update_expired_notes(&db, block.block_height)?;

            Ok(new_witnesses)
        })
    }

    async fn store_received_tx(
        &mut self,
        received_tx: &ReceivedTransaction,
    ) -> Result<Self::TxRef, Self::Error> {
        self.transactionally(|up| {
            let db = up.wallet_db.inner.lock().unwrap();
            let tx_ref = wallet_actions::put_tx_data(&db, received_tx.tx, None)?;

            for output in received_tx.outputs {
                if output.outgoing {
                    wallet_actions::put_sent_note(&db, output, tx_ref)?;
                } else {
                    wallet_actions::put_received_note(&db, output, tx_ref)?;
                }
            }

            Ok(tx_ref)
        })
    }

    async fn store_sent_tx(
        &mut self,
        sent_tx: &SentTransaction,
    ) -> Result<Self::TxRef, Self::Error> {
        // Update the database atomically, to ensure the result is internally consistent.
        self.transactionally(|up| {
            let db = up.wallet_db.inner.lock().unwrap();
            let tx_ref = wallet_actions::put_tx_data(&db, &sent_tx.tx, Some(sent_tx.created))?;

            // Mark notes as spent.
            //
            // This locks the notes so they aren't selected again by a subsequent call to
            // create_spend_to_address() before this transaction has been mined (at which point the notes
            // get re-marked as spent).
            //
            // Assumes that create_spend_to_address() will never be called in parallel, which is a
            // reasonable assumption for a light client such as a mobile phone.
            for spend in &sent_tx.tx.shielded_spends {
                wallet_actions::mark_spent(&db, tx_ref, &spend.nullifier)?;
            }

            wallet_actions::insert_sent_note(
                &db,
                tx_ref,
                sent_tx.output_index,
                sent_tx.account,
                sent_tx.recipient_address,
                sent_tx.value,
                sent_tx.memo.as_ref(),
            )?;

            // Return the row number of the transaction, so the caller can fetch it for sending.
            Ok(tx_ref)
        })
    }

    async fn rewind_to_height(&mut self, block_height: BlockHeight) -> Result<(), Self::Error> {
        let db = self.wallet_db.inner.lock().unwrap();
        wallet::rewind_to_height(&db, block_height)
    }
}
