use crate::address_from_extfvk;
use crate::error::SqliteClientError;
use crate::for_async::{async_blocking, WalletDbAsync};
use rusqlite::ToSql;
use zcash_client_backend::encoding::encode_extended_full_viewing_key;
use zcash_primitives::block::BlockHash;
use zcash_primitives::consensus;
use zcash_primitives::consensus::BlockHeight;
use zcash_primitives::zip32::ExtendedFullViewingKey;

pub async fn init_wallet_db<P: consensus::Parameters + 'static>(
    wdb: &WalletDbAsync<P>,
) -> Result<(), rusqlite::Error>
where
    P: Clone + Send + Sync,
{
    let wdb = wdb.inner.clone();
    async_blocking(move || {
        let wdb = wdb.lock().unwrap();

        wdb.conn.execute(
            "CREATE TABLE IF NOT EXISTS accounts (
            account INTEGER PRIMARY KEY,
            extfvk TEXT NOT NULL,
            address TEXT NOT NULL
        )",
            [],
        )?;
        wdb.conn.execute(
            "CREATE TABLE IF NOT EXISTS blocks (
            height INTEGER PRIMARY KEY,
            hash BLOB NOT NULL,
            time INTEGER NOT NULL,
            sapling_tree BLOB NOT NULL
        )",
            [],
        )?;
        wdb.conn.execute(
            "CREATE TABLE IF NOT EXISTS transactions (
            id_tx INTEGER PRIMARY KEY,
            txid BLOB NOT NULL UNIQUE,
            created TEXT,
            block INTEGER,
            tx_index INTEGER,
            expiry_height INTEGER,
            raw BLOB,
            FOREIGN KEY (block) REFERENCES blocks(height)
        )",
            [],
        )?;
        wdb.conn.execute(
            "CREATE TABLE IF NOT EXISTS received_notes (
            id_note INTEGER PRIMARY KEY,
            tx INTEGER NOT NULL,
            output_index INTEGER NOT NULL,
            account INTEGER NOT NULL,
            diversifier BLOB NOT NULL,
            value INTEGER NOT NULL,
            rcm BLOB NOT NULL,
            nf BLOB NOT NULL UNIQUE,
            is_change INTEGER NOT NULL,
            memo BLOB,
            spent INTEGER,
            FOREIGN KEY (tx) REFERENCES transactions(id_tx),
            FOREIGN KEY (account) REFERENCES accounts(account),
            FOREIGN KEY (spent) REFERENCES transactions(id_tx),
            CONSTRAINT tx_output UNIQUE (tx, output_index)
        )",
            [],
        )?;
        wdb.conn.execute(
            "CREATE TABLE IF NOT EXISTS sapling_witnesses (
            id_witness INTEGER PRIMARY KEY,
            note INTEGER NOT NULL,
            block INTEGER NOT NULL,
            witness BLOB NOT NULL,
            FOREIGN KEY (note) REFERENCES received_notes(id_note),
            FOREIGN KEY (block) REFERENCES blocks(height),
            CONSTRAINT witness_height UNIQUE (note, block)
        )",
            [],
        )?;
        wdb.conn.execute(
            "CREATE TABLE IF NOT EXISTS sent_notes (
            id_note INTEGER PRIMARY KEY,
            tx INTEGER NOT NULL,
            output_index INTEGER NOT NULL,
            from_account INTEGER NOT NULL,
            address TEXT NOT NULL,
            value INTEGER NOT NULL,
            memo BLOB,
            FOREIGN KEY (tx) REFERENCES transactions(id_tx),
            FOREIGN KEY (from_account) REFERENCES accounts(account),
            CONSTRAINT tx_output UNIQUE (tx, output_index)
        )",
            [],
        )?;
        Ok(())
    })
    .await
}

pub async fn init_accounts_table<P: consensus::Parameters + 'static>(
    wdb: &WalletDbAsync<P>,
    extfvks: &[ExtendedFullViewingKey],
) -> Result<(), SqliteClientError>
where
    P: Clone + Send + Sync,
{
    let wdb = wdb.inner.clone();
    let extfvks = extfvks.to_vec();

    async_blocking(move || {
        let wdb = wdb.lock().unwrap();

        let mut empty_check = wdb.conn.prepare("SELECT * FROM accounts LIMIT 1")?;
        if empty_check.exists([])? {
            return Err(SqliteClientError::TableNotEmpty);
        }

        // Insert accounts atomically
        wdb.conn.execute("BEGIN IMMEDIATE", [])?;
        for (account, extfvk) in extfvks.iter().enumerate() {
            let extfvk_str = encode_extended_full_viewing_key(
                wdb.params.hrp_sapling_extended_full_viewing_key(),
                extfvk,
            );

            let address_str = address_from_extfvk(&wdb.params, extfvk);

            wdb.conn.execute(
                "INSERT INTO accounts (account, extfvk, address)
            VALUES (?, ?, ?)",
                [
                    (account as u32).to_sql()?,
                    extfvk_str.to_sql()?,
                    address_str.to_sql()?,
                ],
            )?;
        }
        wdb.conn.execute("COMMIT", [])?;

        Ok(())
    })
    .await
}

pub async fn init_blocks_table<P: consensus::Parameters + 'static>(
    wdb: &WalletDbAsync<P>,
    height: BlockHeight,
    hash: BlockHash,
    time: u32,
    sapling_tree: &[u8],
) -> Result<(), SqliteClientError>
where
    P: Clone + Send + Sync,
{
    let wdb = wdb.inner.clone();
    let sapling_tree = sapling_tree.to_vec().clone();

    async_blocking(move || {
        let wdb = wdb.lock().unwrap();
        let mut empty_check = wdb.conn.prepare("SELECT * FROM blocks LIMIT 1")?;
        if empty_check.exists([])? {
            return Err(SqliteClientError::TableNotEmpty);
        }

        wdb.conn.execute(
            "INSERT INTO blocks (height, hash, time, sapling_tree)
        VALUES (?, ?, ?, ?)",
            [
                u32::from(height).to_sql()?,
                hash.0.to_sql()?,
                time.to_sql()?,
                sapling_tree.as_slice().to_sql()?,
            ],
        )?;

        Ok(())
    })
    .await
}
