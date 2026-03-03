# Antithesis Postgres workload

An Antithesis workload for testing Postgres as well as Postgres-compatible
databases.

This workload is designed so that multiple clients can be run against the same
database.

## Configuration

The Antigres container can be configured using the following environment
variables:

| Variable | Required | Description |
|----------|----------|-------------|
| `DATABASE_URL` | Yes | Comma-separated list of Postgres connection strings. |
| `RW_MODE` | No | `READ` or `WRITE`. In READ mode we only run verification queries (useful for testing read-only replicas). Defaults to `WRITE`. |
| `SELECTED_TESTS` | No | Comma-separated list of tests to run. Options: `bank_test`, `uuids`, `cksum`, `egglog`. Defaults to `bank_test,uuids,cksum`. |
| `MAX_CONNECTIONS` | No | Maximum number of connections per pool. Defaults to `20`. |
| `MAX_RETRIES` | No | Maximum number of non-serialization retries before giving up on an operation. Defaults to `20`. |
| `MAX_SERIALIZATION_RETRIES` | No | Maximum number of serialization-failure retries before giving up on a transaction. Defaults to `7`. |
| `ANTIGRES_KEEP_LOGGING_ON` | No | Set to `1` or `true` to prevent antigres from disabling verbose Postgres logging (`log_connections`, `log_statement`, etc.). |
| `BANK_TEST_TABLE_NAME` | No | Name of the Postgres table used by the `bank_test` workload. Defaults to `accounts`. |
| `BANK_TEST_TXN_LEVEL` | No | Transaction isolation level for the `bank_test` workload. Options: `SERIALIZABLE`, `READ_COMMITTED`. Defaults to `SERIALIZABLE`. |
| `BANK_TEST_EXTRA_SETUP_SQL` | No | Extra SQL to execute during `bank_test` table creation (runs once after the table has been created, in the same transaction). |
| `UUIDS_TABLE_NAME` | No | Name of the Postgres table used by the `uuids` workload. Defaults to `uuids`. |
| `UUIDS_EXTRA_SETUP_SQL` | No | Extra SQL to execute during `uuids` table creation (runs once after the table has been created, in the same transaction). |
| `CKSUM_DATA1_TABLE_NAME` | No | Name of the first data table used by the `cksum` workload. Defaults to `cksum_data1`. |
| `CKSUM_DATA2_TABLE_NAME` | No | Name of the second data table used by the `cksum` workload. Defaults to `cksum_data2`. |
| `CKSUM_DATA3_TABLE_NAME` | No | Name of the third data table used by the `cksum` workload. Defaults to `cksum_data3`. |
| `CKSUMS_TABLE_NAME` | No | Name of the checksum tracking table used by the `cksum` workload. Defaults to `cksums`. |
| `CKSUM_EXTRA_SETUP_SQL` | No | Extra SQL to execute during `cksum` table creation (runs once after the tables have been created, in the same transaction). |

## Available tests

- `bank_test` - this test creates a table
                `accounts (id INT PRIMARY KEY, balance INTEGER NOT NULL)`. We
                initialize this table with a randomly chosen number of accounts,
                and then conduct transfers of randomly-chosen amounts between 
                these accounts.
    - **Invariant 1**: the total sum across all accounts is constant.
    - **Invariant 2**: no account has a negative balance.
- `uuids`     - this test creates a table `uuids ( value TEXT NOT NULL UNIQUE )`.
                In the workload, we pick random UUID v4s and insert them into
                the database.
    - **Invariant**: we run a thread that fetches stored UUIDs from the database
                     and continually checks that no UUIDs are deleted once insertion
                     has been confirmed.
- `cksum` - this test creates three tables, and stores the sum of each of the
            checksums of all of the rows in each table in a separate table. As we
            carry out operations (e.g. adding/deleting data, moving an item from
            one table to another) we update the stored checksum.
    - **Invariant**: The validation
            checks that the checksum stored in the database is equivalent to directly
            computing the sum of the checksums of each item in the table.