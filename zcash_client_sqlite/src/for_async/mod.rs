pub mod init;
pub mod wallet_actions;

use std::collections::HashMap;
use std::path::Path;
use zcash_client_backend::data_api::{PrunedBlock, ReceivedTransaction, SentTransaction};
use zcash_client_backend::wallet::{AccountId, SpendableNote};
use zcash_extras::{WalletRead, WalletWrite};
use zcash_primitives::block::BlockHash;
use zcash_primitives::consensus::BlockHeight;
use zcash_primitives::memo::Memo;
use zcash_primitives::merkle_tree::{CommitmentTree, IncrementalWitness};
use zcash_primitives::sapling::{Node, Nullifier, PaymentAddress};
use zcash_primitives::transaction::components::Amount;
use zcash_primitives::transaction::TxId;
use zcash_primitives::zip32::ExtendedFullViewingKey;

pub async fn async_blocking<F, R>(blocking_fn: F) -> R
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    tokio::task::spawn_blocking(blocking_fn)
        .await
        .expect("spawn_blocking to succeed")
}

use crate::error::SqliteClientError;
use crate::{wallet, NoteId, WalletDb};
use rusqlite::Connection;
use std::sync::{Arc, Mutex};

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
        let db = self.clone();
        async_blocking(move || {
            let db = db.inner.lock().unwrap();
            wallet::block_height_extrema(&db).map_err(SqliteClientError::from)
        })
        .await
    }

    async fn get_block_hash(
        &self,
        block_height: BlockHeight,
    ) -> Result<Option<BlockHash>, Self::Error> {
        let db = self.clone();
        async_blocking(move || {
            let db = db.inner.lock().unwrap();
            wallet::get_block_hash(&db, block_height).map_err(SqliteClientError::from)
        })
        .await
    }

    async fn get_tx_height(&self, txid: TxId) -> Result<Option<BlockHeight>, Self::Error> {
        let db = self.clone();
        async_blocking(move || {
            let db = db.inner.lock().unwrap();
            wallet::get_tx_height(&db, txid).map_err(SqliteClientError::from)
        })
        .await
    }

    async fn get_address(&self, account: AccountId) -> Result<Option<PaymentAddress>, Self::Error> {
        let db = self.clone();
        async_blocking(move || {
            let db = db.inner.lock().unwrap();
            wallet::get_address(&db, account).map_err(SqliteClientError::from)
        })
        .await
    }

    async fn get_extended_full_viewing_keys(
        &self,
    ) -> Result<HashMap<AccountId, ExtendedFullViewingKey>, Self::Error> {
        let db = self.clone();
        async_blocking(move || {
            let db = db.inner.lock().unwrap();
            wallet::get_extended_full_viewing_keys(&db).map_err(SqliteClientError::from)
        })
        .await
    }

    async fn is_valid_account_extfvk(
        &self,
        account: AccountId,
        extfvk: &ExtendedFullViewingKey,
    ) -> Result<bool, Self::Error> {
        let db = self.clone();
        let extfvk = extfvk.clone();
        async_blocking(move || {
            let db = db.inner.lock().unwrap();
            wallet::is_valid_account_extfvk(&db, account, &extfvk)
        })
        .await
    }

    async fn get_balance_at(
        &self,
        account: AccountId,
        anchor_height: BlockHeight,
    ) -> Result<Amount, Self::Error> {
        let db = self.clone();
        async_blocking(move || {
            let db = db.inner.lock().unwrap();
            wallet::get_balance_at(&db, account, anchor_height)
        })
        .await
    }

    async fn get_memo(&self, id_note: Self::NoteRef) -> Result<Memo, Self::Error> {
        let db = self.clone();
        async_blocking(move || {
            let db = db.inner.lock().unwrap();
            match id_note {
                NoteId::SentNoteId(id_note) => wallet::get_sent_memo(&db, id_note),
                NoteId::ReceivedNoteId(id_note) => wallet::get_received_memo(&db, id_note),
            }
        })
        .await
    }

    async fn get_commitment_tree(
        &self,
        block_height: BlockHeight,
    ) -> Result<Option<CommitmentTree<Node>>, Self::Error> {
        let db = self.clone();
        async_blocking(move || {
            let db = db.inner.lock().unwrap();
            wallet::get_commitment_tree(&db, block_height)
        })
        .await
    }

    #[allow(clippy::type_complexity)]
    async fn get_witnesses(
        &self,
        block_height: BlockHeight,
    ) -> Result<Vec<(Self::NoteRef, IncrementalWitness<Node>)>, Self::Error> {
        let db = self.clone();
        async_blocking(move || {
            let db = db.inner.lock().unwrap();
            wallet::get_witnesses(&db, block_height)
        })
        .await
    }

    async fn get_nullifiers(&self) -> Result<Vec<(AccountId, Nullifier)>, Self::Error> {
        let db = self.clone();
        async_blocking(move || {
            let db = db.inner.lock().unwrap();
            wallet::get_nullifiers(&db)
        })
        .await
    }

    async fn get_spendable_notes(
        &self,
        account: AccountId,
        anchor_height: BlockHeight,
    ) -> Result<Vec<SpendableNote>, Self::Error> {
        let db = self.clone();
        async_blocking(move || {
            let db = db.inner.lock().unwrap();
            wallet::transact::get_spendable_notes(&db, account, anchor_height)
        })
        .await
    }

    async fn select_spendable_notes(
        &self,
        account: AccountId,
        target_value: Amount,
        anchor_height: BlockHeight,
    ) -> Result<Vec<SpendableNote>, Self::Error> {
        let db = self.clone();
        async_blocking(move || {
            let db = db.inner.lock().unwrap();
            wallet::transact::select_spendable_notes(&db, account, target_value, anchor_height)
        })
        .await
    }
}

#[derive(Clone)]
pub struct DataConnStmtCacheAsync<P> {
    wallet_db: WalletDbAsync<P>,
}

impl<P: consensus::Parameters> DataConnStmtCacheAsync<P> {
    fn transactionally<F, A>(self, f: F) -> Result<A, SqliteClientError>
    where
        F: FnOnce(&Self) -> Result<A, SqliteClientError>,
    {
        self.wallet_db
            .inner
            .lock()
            .unwrap()
            .conn
            .execute("BEGIN IMMEDIATE", [])?;
        match f(&self) {
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
        self.wallet_db.block_height_extrema().await
    }

    async fn get_block_hash(
        &self,
        block_height: BlockHeight,
    ) -> Result<Option<BlockHash>, Self::Error> {
        self.wallet_db.get_block_hash(block_height).await
    }

    async fn get_tx_height(&self, txid: TxId) -> Result<Option<BlockHeight>, Self::Error> {
        self.wallet_db.get_tx_height(txid).await
    }

    async fn get_address(&self, account: AccountId) -> Result<Option<PaymentAddress>, Self::Error> {
        self.wallet_db.get_address(account).await
    }

    async fn get_extended_full_viewing_keys(
        &self,
    ) -> Result<HashMap<AccountId, ExtendedFullViewingKey>, Self::Error> {
        self.wallet_db.get_extended_full_viewing_keys().await
    }

    async fn is_valid_account_extfvk(
        &self,
        account: AccountId,
        extfvk: &ExtendedFullViewingKey,
    ) -> Result<bool, Self::Error> {
        self.wallet_db
            .is_valid_account_extfvk(account, extfvk)
            .await
    }

    async fn get_balance_at(
        &self,
        account: AccountId,
        anchor_height: BlockHeight,
    ) -> Result<Amount, Self::Error> {
        self.wallet_db.get_balance_at(account, anchor_height).await
    }

    async fn get_memo(&self, id_note: Self::NoteRef) -> Result<Memo, Self::Error> {
        self.wallet_db.get_memo(id_note).await
    }

    async fn get_commitment_tree(
        &self,
        block_height: BlockHeight,
    ) -> Result<Option<CommitmentTree<Node>>, Self::Error> {
        self.wallet_db.get_commitment_tree(block_height).await
    }

    #[allow(clippy::type_complexity)]
    async fn get_witnesses(
        &self,
        block_height: BlockHeight,
    ) -> Result<Vec<(Self::NoteRef, IncrementalWitness<Node>)>, Self::Error> {
        self.wallet_db.get_witnesses(block_height).await
    }

    async fn get_nullifiers(&self) -> Result<Vec<(AccountId, Nullifier)>, Self::Error> {
        self.wallet_db.get_nullifiers().await
    }

    async fn get_spendable_notes(
        &self,
        account: AccountId,
        anchor_height: BlockHeight,
    ) -> Result<Vec<SpendableNote>, Self::Error> {
        self.wallet_db
            .get_spendable_notes(account, anchor_height)
            .await
    }

    async fn select_spendable_notes(
        &self,
        account: AccountId,
        target_value: Amount,
        anchor_height: BlockHeight,
    ) -> Result<Vec<SpendableNote>, Self::Error> {
        self.wallet_db
            .select_spendable_notes(account, target_value, anchor_height)
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
        self.clone().transactionally(|up| {
            let db = up.wallet_db.inner.lock().unwrap();
            // Insert the block into the database.
            wallet_actions::insert_block(
                &db,
                block.block_height,
                block.block_hash,
                block.block_time,
                block.commitment_tree,
            )?;

            let mut new_witnesses = vec![];
            for tx in block.transactions {
                let tx_row = wallet_actions::put_tx_meta(&db, tx, block.block_height)?;

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
        self.clone().transactionally(|up| {
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
        self.clone().transactionally(|up| {
            let db = up.wallet_db.inner.lock().unwrap();
            let tx_ref = wallet_actions::put_tx_data(&db, sent_tx.tx, Some(sent_tx.created))?;

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
        let db = self.clone();
        async_blocking(move || {
            let db = db.wallet_db.inner.lock().unwrap();
            wallet::rewind_to_height(&db, block_height)
        })
        .await
    }
}
