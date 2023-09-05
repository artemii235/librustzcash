use crate::error::SqliteClientError;
use crate::wallet::ShieldedOutput;
use crate::{NoteId, WalletDb};
use ff::PrimeField;
use rusqlite::{params, ToSql};
use std::sync::MutexGuard;
use zcash_client_backend::address::RecipientAddress;
use zcash_client_backend::encoding::encode_payment_address;
use zcash_client_backend::wallet::{AccountId, WalletTx};
use zcash_client_backend::DecryptedOutput;
use zcash_primitives::block::BlockHash;
use zcash_primitives::consensus;
use zcash_primitives::consensus::BlockHeight;
use zcash_primitives::memo::MemoBytes;
use zcash_primitives::merkle_tree::{CommitmentTree, IncrementalWitness};
use zcash_primitives::sapling::{Node, Nullifier};
use zcash_primitives::transaction::components::Amount;
use zcash_primitives::transaction::Transaction;

/// Inserts information about a scanned block into the database.
pub fn insert_block<P>(
    db: &MutexGuard<WalletDb<P>>,
    block_height: BlockHeight,
    block_hash: BlockHash,
    block_time: u32,
    commitment_tree: &CommitmentTree<Node>,
) -> Result<(), SqliteClientError> {
    let mut encoded_tree = Vec::new();

    commitment_tree.write(&mut encoded_tree).unwrap();
    db.conn
        .prepare(
            "INSERT INTO blocks (height, hash, time, sapling_tree)
                    VALUES (?, ?, ?, ?)",
        )?
        .execute(params![
            u32::from(block_height),
            &block_hash.0[..],
            block_time,
            encoded_tree
        ])?;

    Ok(())
}

/// Inserts information about a mined transaction that was observed to
/// contain a note related to this wallet into the database.
pub fn put_tx_meta<P, N>(
    db: &MutexGuard<WalletDb<P>>,
    tx: &WalletTx<N>,
    height: BlockHeight,
) -> Result<i64, SqliteClientError> {
    let txid = tx.txid.0.to_vec();
    if db
        .conn
        .prepare(
            "UPDATE transactions
                    SET block = ?, tx_index = ? WHERE txid = ?",
        )?
        .execute(params![u32::from(height), (tx.index as i64), txid])?
        == 0
    {
        // It isn't there, so insert our transaction into the database.
        db.conn
            .prepare(
                "INSERT INTO transactions (txid, block, tx_index)
                    VALUES (?, ?, ?)",
            )?
            .execute(params![txid, u32::from(height), (tx.index as i64),])?;

        Ok(db.conn.last_insert_rowid())
    } else {
        // It was there, so grab its row number.
        db.conn
            .prepare("SELECT id_tx FROM transactions WHERE txid = ?")?
            .query_row([txid], |row| row.get(0))
            .map_err(SqliteClientError::from)
    }
}

/// Marks a given nullifier as having been revealed in the construction
/// of the specified transaction.
///
/// Marking a note spent in this fashion does NOT imply that the
/// spending transaction has been mined.
pub fn mark_spent<P>(
    db: &MutexGuard<WalletDb<P>>,
    tx_ref: i64,
    nf: &Nullifier,
) -> Result<(), SqliteClientError> {
    db.conn
        .prepare("UPDATE received_notes SET spent = ? WHERE nf = ?")?
        .execute([tx_ref.to_sql()?, nf.0.to_sql()?])?;
    Ok(())
}

/// Records the specified shielded output as having been received.
// Assumptions:
// - A transaction will not contain more than 2^63 shielded outputs.
// - A note value will never exceed 2^63 zatoshis.
pub fn put_received_note<P, T: ShieldedOutput>(
    db: &MutexGuard<WalletDb<P>>,
    output: &T,
    tx_ref: i64,
) -> Result<NoteId, SqliteClientError> {
    let rcm = output.note().rcm().to_repr();
    let account = output.account().0 as i64;
    let diversifier = output.to().diversifier().0.to_vec();
    let value = output.note().value as i64;
    let rcm = rcm.as_ref();
    let memo = output.memo().map(|m| m.as_slice());
    let is_change = output.is_change();
    let tx = tx_ref;
    let output_index = output.index() as i64;
    let nf_bytes = output.nullifier().map(|nf| nf.0.to_vec());

    let sql_args: &[(&str, &dyn ToSql)] = &[
        (":account", &account),
        (":diversifier", &diversifier),
        (":value", &value),
        (":rcm", &rcm),
        (":nf", &nf_bytes),
        (":memo", &memo),
        (":is_change", &is_change),
        (":tx", &tx),
        (":output_index", &output_index),
    ];

    // First try updating an existing received note into the database.
    if db
        .conn
        .prepare(
            "UPDATE received_notes
                    SET account = :account,
                        diversifier = :diversifier,
                        value = :value,
                        rcm = :rcm,
                        nf = IFNULL(:nf, nf),
                        memo = IFNULL(:memo, memo),
                        is_change = IFNULL(:is_change, is_change)
                    WHERE tx = :tx AND output_index = :output_index",
        )?
        .execute(sql_args)?
        == 0
    {
        // It isn't there, so insert our note into the database.
        db.conn
            .prepare(
                "UPDATE received_notes
                    SET account = :account,
                        diversifier = :diversifier,
                        value = :value,
                        rcm = :rcm,
                        nf = IFNULL(:nf, nf),
                        memo = IFNULL(:memo, memo),
                        is_change = IFNULL(:is_change, is_change)
                    WHERE tx = :tx AND output_index = :output_index",
            )?
            .execute(sql_args)?;

        Ok(NoteId::ReceivedNoteId(db.conn.last_insert_rowid()))
    } else {
        // It was there, so grab its row number.
        db.conn
            .prepare("SELECT id_note FROM received_notes WHERE tx = ? AND output_index = ?")?
            .query_row(params![tx_ref, (output.index() as i64)], |row| {
                row.get(0).map(NoteId::ReceivedNoteId)
            })
            .map_err(SqliteClientError::from)
    }
}

/// Records the incremental witness for the specified note,
/// as of the given block height.
pub fn insert_witness<P>(
    db: &MutexGuard<WalletDb<P>>,
    note_id: i64,
    witness: &IncrementalWitness<Node>,
    height: BlockHeight,
) -> Result<(), SqliteClientError> {
    let mut encoded = Vec::new();
    witness.write(&mut encoded).unwrap();

    db.conn
        .prepare(
            "INSERT INTO sapling_witnesses (note, block, witness)
                    VALUES (?, ?, ?)",
        )?
        .execute(params![note_id, u32::from(height), encoded])?;

    Ok(())
}

/// Removes old incremental witnesses up to the given block height.
pub fn prune_witnesses<P>(
    db: &MutexGuard<WalletDb<P>>,
    below_height: BlockHeight,
) -> Result<(), SqliteClientError> {
    db.conn
        .prepare("DELETE FROM sapling_witnesses WHERE block < ?")?
        .execute([u32::from(below_height)])?;
    Ok(())
}

/// Marks notes that have not been mined in transactions
/// as expired, up to the given block height.
pub fn update_expired_notes<P>(
    db: &MutexGuard<WalletDb<P>>,
    height: BlockHeight,
) -> Result<(), SqliteClientError> {
    db.conn
        .prepare(
            "UPDATE received_notes SET spent = NULL WHERE EXISTS (
                        SELECT id_tx FROM transactions
                        WHERE id_tx = received_notes.spent AND block IS NULL AND expiry_height < ?
                    )",
        )?
        .execute([u32::from(height)])?;
    Ok(())
}

/// Inserts full transaction data into the database.
pub fn put_tx_data<P>(
    db: &MutexGuard<WalletDb<P>>,
    tx: &Transaction,
    created_at: Option<time::OffsetDateTime>,
) -> Result<i64, SqliteClientError> {
    let txid = tx.txid().0.to_vec();

    let mut raw_tx = vec![];
    tx.write(&mut raw_tx)?;

    if db
        .conn
        .prepare(
            "UPDATE transactions
                    SET expiry_height = ?, raw = ? WHERE txid = ?",
        )?
        .execute(params![u32::from(tx.expiry_height), raw_tx, txid,])?
        == 0
    {
        // It isn't there, so insert our transaction into the database.
        db.conn
            .prepare(
                "INSERT INTO transactions (txid, created, expiry_height, raw)
                    VALUES (?, ?, ?, ?)",
            )?
            .execute(params![
                txid,
                created_at.,
                u32::from(tx.expiry_height),
                raw_tx
            ])?;

        Ok(db.conn.last_insert_rowid())
    } else {
        // It was there, so grab its row number.
        db.conn
            .prepare("SELECT id_tx FROM transactions WHERE txid = ?")?
            .query_row([txid], |row| row.get(0))
            .map_err(SqliteClientError::from)
    }
}

/// Records information about a note that your wallet created.
pub fn put_sent_note<P: consensus::Parameters>(
    db: &MutexGuard<WalletDb<P>>,
    output: &DecryptedOutput,
    tx_ref: i64,
) -> Result<(), SqliteClientError> {
    let output_index = output.index as i64;
    let account = output.account.0 as i64;
    let value = output.note.value as i64;
    let to_str = encode_payment_address(db.params.hrp_sapling_payment_address(), &output.to);

    // Try updating an existing sent note.
    if db
        .conn
        .prepare(
            "UPDATE sent_notes
                    SET from_account = ?, address = ?, value = ?, memo = ?
                    WHERE tx = ? AND output_index = ?",
        )?
        .execute(params![
            account,
            to_str,
            value,
            &output.memo.as_slice(),
            tx_ref,
            output_index
        ])?
        == 0
    {
        // It isn't there, so insert.
        insert_sent_note(
            db,
            tx_ref,
            output.index,
            output.account,
            &RecipientAddress::Shielded(output.to.clone()),
            Amount::from_u64(output.note.value)
                .map_err(|_| SqliteClientError::CorruptedData("Note value invalid.".to_string()))?,
            Some(&output.memo),
        )?
    }

    Ok(())
}

/// Inserts a sent note into the wallet database.
///
/// `output_index` is the index within the transaction that contains the recipient output:
///
/// - If `to` is a Sapling address, this is an index into the Sapling outputs of the
///   transaction.
/// - If `to` is a transparent address, this is an index into the transparent outputs of
///   the transaction.
pub fn insert_sent_note<P: consensus::Parameters>(
    db: &MutexGuard<WalletDb<P>>,
    tx_ref: i64,
    output_index: usize,
    account: AccountId,
    to: &RecipientAddress,
    value: Amount,
    memo: Option<&MemoBytes>,
) -> Result<(), SqliteClientError> {
    let to_str = to.encode(&db.params);
    let ivalue: i64 = value.into();
    db.conn
        .prepare(
            "INSERT INTO sent_notes (tx, output_index, from_account, address, value, memo)
                    VALUES (?, ?, ?, ?, ?, ?)",
        )?
        .execute(params![
            tx_ref,
            (output_index as i64),
            account.0,
            to_str,
            ivalue,
            memo.map(|m| m.as_slice().to_vec()),
        ])?;

    Ok(())
}
