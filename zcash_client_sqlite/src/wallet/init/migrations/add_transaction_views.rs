//! Migration that adds transaction summary views & add fee information to transactions.
use std::collections::HashSet;

use rusqlite::{self, types::ToSql, OptionalExtension, NO_PARAMS};
use schemer::{self};
use schemer_rusqlite::RusqliteMigration;
use uuid::Uuid;

use zcash_primitives::{
    consensus::{self, BlockHeight, BranchId},
    transaction::{components::amount::Amount, Transaction},
};

use super::{ufvk_support, utxos_table};
use crate::wallet::init::WalletMigrationError;

pub(super) const MIGRATION_ID: Uuid = Uuid::from_fields(
    0x282fad2e,
    0x8372,
    0x4ca0,
    b"\x8b\xed\x71\x82\x13\x20\x90\x9f",
);

pub(crate) struct Migration<P> {
    pub(super) params: P,
}

impl<P> schemer::Migration for Migration<P> {
    fn id(&self) -> Uuid {
        MIGRATION_ID
    }

    fn dependencies(&self) -> HashSet<Uuid> {
        [ufvk_support::MIGRATION_ID, utxos_table::MIGRATION_ID]
            .into_iter()
            .collect()
    }

    fn description(&self) -> &'static str {
        "Add transaction summary views & add fee information to transactions."
    }
}

impl<P: consensus::Parameters> RusqliteMigration for Migration<P> {
    type Error = WalletMigrationError;

    fn up(&self, transaction: &rusqlite::Transaction) -> Result<(), WalletMigrationError> {
        transaction.execute_batch("ALTER TABLE transactions ADD COLUMN fee INTEGER;")?;

        let mut stmt_list_txs =
            transaction.prepare("SELECT id_tx, raw, block FROM transactions")?;

        let mut stmt_set_fee =
            transaction.prepare("UPDATE transactions SET fee = ? WHERE id_tx = ?")?;

        let mut stmt_find_utxo_value = transaction
            .prepare("SELECT value_zat FROM utxos WHERE prevout_txid = ? AND prevout_idx = ?")?;

        let mut tx_rows = stmt_list_txs.query(NO_PARAMS)?;
        while let Some(row) = tx_rows.next()? {
            let id_tx: i64 = row.get(0)?;
            let tx_bytes: Option<Vec<u8>> = row.get(1)?;
            let h: u32 = row.get(2)?;
            let block_height = BlockHeight::from(h);

            // If only transaction metadata has been stored, and not transaction data, the fee
            // information will eventually be set when the full transaction data is inserted.
            if let Some(b) = tx_bytes {
                let tx =
                    Transaction::read(&b[..], BranchId::for_height(&self.params, block_height))
                        .map_err(|e| {
                            WalletMigrationError::CorruptedData(format!(
                                "Parsing failed for transaction {:?}: {:?}",
                                id_tx, e
                            ))
                        })?;

                let fee_paid = tx.fee_paid(|op| {
                    let op_amount = stmt_find_utxo_value
                        .query_row(&[op.hash().to_sql()?, op.n().to_sql()?], |row| {
                            row.get::<_, i64>(0)
                        })
                        .optional()
                        .map_err(WalletMigrationError::DbError)?;

                    op_amount.map_or_else(
                        || {
                            Err(WalletMigrationError::CorruptedData(format!(
                                "Unable to find UTXO corresponding to outpoint {:?}",
                                op
                            )))
                        },
                        |i| {
                            Amount::from_i64(i).map_err(|_| {
                                WalletMigrationError::CorruptedData(format!(
                                    "UTXO amount out of range in outpoint {:?}",
                                    op
                                ))
                            })
                        },
                    )
                })?;

                stmt_set_fee.execute(&[i64::from(fee_paid), id_tx])?;
            }
        }

        transaction.execute_batch(
            "UPDATE sent_notes SET memo = NULL
              WHERE memo = X'F600000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000';
            UPDATE received_notes SET memo = NULL
              WHERE memo = X'F600000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000';")?;

        transaction.execute_batch(
            "CREATE VIEW v_tx_sent AS
            SELECT transactions.id_tx         AS id_tx,
                   transactions.block         AS mined_height,
                   transactions.tx_index      AS tx_index,
                   transactions.txid          AS txid,
                   transactions.expiry_height AS expiry_height,
                   transactions.raw           AS raw,
                   SUM(sent_notes.value)      AS sent_total,
                   COUNT(sent_notes.id_note)  AS sent_note_count,
                   SUM(
                       CASE
                           WHEN sent_notes.memo IS NULL THEN 0
                           ELSE 1
                       END
                   ) AS memo_count,
                   blocks.time                AS block_time
            FROM   transactions
                   JOIN sent_notes
                          ON transactions.id_tx = sent_notes.tx
                   LEFT JOIN blocks
                          ON transactions.block = blocks.height
            GROUP BY sent_notes.tx;
            CREATE VIEW v_tx_received AS
            SELECT transactions.id_tx            AS id_tx,
                   transactions.block            AS mined_height,
                   transactions.tx_index         AS tx_index,
                   transactions.txid             AS txid,
                   SUM(received_notes.value)     AS received_total,
                   COUNT(received_notes.id_note) AS received_note_count,
                   SUM(
                       CASE
                           WHEN received_notes.memo IS NULL THEN 0
                           ELSE 1
                       END
                   ) AS memo_count,
                   blocks.time                   AS block_time
            FROM   transactions
                   JOIN received_notes
                          ON transactions.id_tx = received_notes.tx
                   LEFT JOIN blocks
                          ON transactions.block = blocks.height
            GROUP BY received_notes.tx;
            CREATE VIEW v_transactions AS
            SELECT id_tx,
                   mined_height,
                   tx_index,
                   txid,
                   expiry_height,
                   raw,
                   SUM(value) + MAX(fee) AS net_value,
                   SUM(is_change) > 0 AS has_change,
                   SUM(memo_present) AS memo_count
            FROM (
                SELECT transactions.id_tx            AS id_tx,
                       transactions.block            AS mined_height,
                       transactions.tx_index         AS tx_index,
                       transactions.txid             AS txid,
                       transactions.expiry_height    AS expiry_height,
                       transactions.raw              AS raw,
                       0                             AS fee,
                       CASE
                            WHEN received_notes.is_change THEN 0
                            ELSE value
                       END AS value,
                       received_notes.is_change      AS is_change,
                       CASE
                           WHEN received_notes.memo IS NULL THEN 0
                           ELSE 1
                       END AS memo_present
                FROM   transactions
                       JOIN received_notes ON transactions.id_tx = received_notes.tx
                UNION
                SELECT transactions.id_tx            AS id_tx,
                       transactions.block            AS mined_height,
                       transactions.tx_index         AS tx_index,
                       transactions.txid             AS txid,
                       transactions.expiry_height    AS expiry_height,
                       transactions.raw              AS raw,
                       transactions.fee              AS fee,
                       -sent_notes.value             AS value,
                       false                         AS is_change,
                       CASE
                           WHEN sent_notes.memo IS NULL THEN 0
                           ELSE 1
                       END AS memo_present
                FROM   transactions
                       JOIN sent_notes ON transactions.id_tx = sent_notes.tx
            )
            GROUP BY id_tx;",
        )?;

        Ok(())
    }

    fn down(&self, _transaction: &rusqlite::Transaction) -> Result<(), WalletMigrationError> {
        // TODO: something better than just panic?
        panic!("Cannot revert this migration.");
    }
}

#[cfg(test)]
mod tests {
    use rusqlite::{self, NO_PARAMS};
    use tempfile::NamedTempFile;

    #[cfg(feature = "transparent-inputs")]
    use {
        crate::wallet::init::migrations::ufvk_support,
        rusqlite::params,
        zcash_client_backend::{encoding::AddressCodec, keys::UnifiedSpendingKey},
        zcash_primitives::{
            consensus::{BlockHeight, BranchId},
            legacy::{keys::IncomingViewingKey, Script},
            transaction::{
                components::{
                    transparent::{self, Authorized, OutPoint},
                    Amount, TxIn, TxOut,
                },
                TransactionData, TxVersion,
            },
            zip32::AccountId,
        },
    };

    use crate::{
        tests,
        wallet::init::{init_wallet_db, init_wallet_db_internal, migrations::addresses_table},
        WalletDb,
    };

    #[test]
    fn transaction_views() {
        let data_file = NamedTempFile::new().unwrap();
        let mut db_data = WalletDb::for_path(data_file.path(), tests::network()).unwrap();
        init_wallet_db_internal(&mut db_data, None, Some(addresses_table::MIGRATION_ID)).unwrap();

        db_data.conn.execute_batch(
            "INSERT INTO accounts (account, ufvk) VALUES (0, '');
            INSERT INTO blocks (height, hash, time, sapling_tree) VALUES (0, 0, 0, '');
            INSERT INTO transactions (block, id_tx, txid) VALUES (0, 0, '');

            INSERT INTO sent_notes (tx, output_pool, output_index, from_account, address, value)
            VALUES (0, 2, 0, 0, '', 2);
            INSERT INTO sent_notes (tx, output_pool, output_index, from_account, address, value, memo)
            VALUES (0, 2, 1, 0, '', 3, X'61');
            INSERT INTO sent_notes (tx, output_pool, output_index, from_account, address, value, memo)
            VALUES (0, 2, 2, 0, '', 0, X'F600000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000');

            INSERT INTO received_notes (tx, output_index, account, diversifier, value, rcm, nf, is_change)
            VALUES (0, 0, 0, '', 2, '', 'a', false);
            INSERT INTO received_notes (tx, output_index, account, diversifier, value, rcm, nf, is_change, memo)
            VALUES (0, 3, 0, '', 5, '', 'b', false, X'62');
            INSERT INTO received_notes (tx, output_index, account, diversifier, value, rcm, nf, is_change, memo)
            VALUES (0, 4, 0, '', 7, '', 'c', true, X'63');",
        ).unwrap();

        init_wallet_db(&mut db_data, None).unwrap();

        let mut q = db_data
            .conn
            .prepare("SELECT received_total, received_note_count, memo_count FROM v_tx_received")
            .unwrap();
        let mut rows = q.query(NO_PARAMS).unwrap();
        let mut row_count = 0;
        while let Some(row) = rows.next().unwrap() {
            row_count += 1;
            let total: i64 = row.get(0).unwrap();
            let count: i64 = row.get(1).unwrap();
            let memo_count: i64 = row.get(2).unwrap();
            assert_eq!(total, 14);
            assert_eq!(count, 3);
            assert_eq!(memo_count, 2);
        }
        assert_eq!(row_count, 1);

        let mut q = db_data
            .conn
            .prepare("SELECT sent_total, sent_note_count, memo_count FROM v_tx_sent")
            .unwrap();
        let mut rows = q.query(NO_PARAMS).unwrap();
        let mut row_count = 0;
        while let Some(row) = rows.next().unwrap() {
            row_count += 1;
            let total: i64 = row.get(0).unwrap();
            let count: i64 = row.get(1).unwrap();
            let memo_count: i64 = row.get(2).unwrap();
            assert_eq!(total, 5);
            assert_eq!(count, 3);
            assert_eq!(memo_count, 1);
        }
        assert_eq!(row_count, 1);

        let mut q = db_data
            .conn
            .prepare("SELECT net_value, has_change, memo_count FROM v_transactions")
            .unwrap();
        let mut rows = q.query(NO_PARAMS).unwrap();
        let mut row_count = 0;
        while let Some(row) = rows.next().unwrap() {
            row_count += 1;
            let net_value: i64 = row.get(0).unwrap();
            let has_change: bool = row.get(1).unwrap();
            let memo_count: i64 = row.get(2).unwrap();
            assert_eq!(net_value, 2);
            assert!(has_change);
            assert_eq!(memo_count, 3);
        }
        assert_eq!(row_count, 1);
    }

    #[test]
    #[cfg(feature = "transparent-inputs")]
    fn migrate_from_wm2() {
        let data_file = NamedTempFile::new().unwrap();
        let mut db_data = WalletDb::for_path(data_file.path(), tests::network()).unwrap();
        init_wallet_db_internal(&mut db_data, None, Some(ufvk_support::MIGRATION_ID)).unwrap();

        // create a UTXO to spend
        let tx = TransactionData::from_parts(
            TxVersion::Sapling,
            BranchId::Canopy,
            0,
            BlockHeight::from(3),
            Some(transparent::Bundle {
                vin: vec![TxIn {
                    prevout: OutPoint::new([1u8; 32], 1),
                    script_sig: Script(vec![]),
                    sequence: 0,
                }],
                vout: vec![TxOut {
                    value: Amount::from_i64(1100000000).unwrap(),
                    script_pubkey: Script(vec![]),
                }],
                authorization: Authorized,
            }),
            None,
            None,
            None,
        )
        .freeze()
        .unwrap();

        let mut tx_bytes = vec![];
        tx.write(&mut tx_bytes).unwrap();

        let usk =
            UnifiedSpendingKey::from_seed(&tests::network(), &[0u8; 32][..], AccountId::from(0))
                .unwrap();
        let ufvk = usk.to_unified_full_viewing_key();
        let (ua, _) = ufvk.default_address();
        let taddr = ufvk
            .transparent()
            .and_then(|k| {
                k.derive_external_ivk()
                    .ok()
                    .map(|k| k.derive_address(0).unwrap())
            })
            .map(|a| a.encode(&tests::network()));

        db_data.conn.execute(
            "INSERT INTO accounts (account, ufvk, address, transparent_address) VALUES (0, ?, ?, ?)",
            params![ufvk.encode(&tests::network()), ua.encode(&tests::network()), &taddr]
        ).unwrap();
        db_data
            .conn
            .execute_batch(
                "INSERT INTO blocks (height, hash, time, sapling_tree) VALUES (0, 0, 0, '');",
            )
            .unwrap();
        db_data.conn.execute(
            "INSERT INTO utxos (address, prevout_txid, prevout_idx, script, value_zat, height)
            VALUES (?, X'0101010101010101010101010101010101010101010101010101010101010101', 1, X'', 1400000000, 1)",
            &[taddr]
        ).unwrap();
        db_data
            .conn
            .execute(
                "INSERT INTO transactions (block, id_tx, txid, raw) VALUES (0, 0, '', ?)",
                params![tx_bytes],
            )
            .unwrap();

        init_wallet_db(&mut db_data, None).unwrap();

        let fee = db_data
            .conn
            .query_row(
                "SELECT fee FROM transactions WHERE id_tx = 0",
                NO_PARAMS,
                |row| Ok(Amount::from_i64(row.get(0)?).unwrap()),
            )
            .unwrap();

        assert_eq!(fee, Amount::from_i64(300000000).unwrap());
    }
}
