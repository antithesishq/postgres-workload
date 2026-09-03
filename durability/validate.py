from antithesis.assertions import always, reachable, unreachable
from antithesis.random import get_random
import psycopg
import sqlite3

from durability.helper import BATCH_SIZE, PRESENT, open_sqlite, text
from test_client.database import Database


def validate_one(db: Database) -> bool:
    """
    Pick a random present key and check that it exists in the database.

    We delete the key from the database to prevent other drivers from deleting
    it in the meantime.
    """
    local_db = open_sqlite()
    try:
        with local_db:
            local_db.execute("BEGIN IMMEDIATE")
            count = local_db.execute(
                "SELECT COUNT(*) FROM records WHERE status = ?", (PRESENT,)
            ).fetchone()[0]
            digest = None
            if count:
                offset = get_random() % count
                digest = local_db.execute(
                    """
                    SELECT cksum
                    FROM records
                    WHERE status = ?
                    ORDER BY cksum
                    LIMIT 1 OFFSET ?
                    """,
                    (PRESENT, offset),
                ).fetchone()[0]
                local_db.execute(
                    "DELETE FROM records WHERE cksum = ?", (digest,)
                )
    except (sqlite3.Error, OSError) as error:
        local_db.close()
        unreachable(
            "Durability anytime: claiming a value from the local ledger failed",
            {"error": str(error)},
        )
        return False

    if digest is None:
        local_db.close()
        return True

    # now check that the key exists in the database
    database_error = None
    try:
        with db.connect() as connection:
            exists = connection.execute(
                'SELECT EXISTS (SELECT 1 FROM "values" WHERE cksum = %s)',
                (digest,),
            ).fetchone()[0]
    except (psycopg.Error, OSError) as error:
        database_error = error

    # add the key back in the database
    try:
        with local_db:
            local_db.execute(
                "INSERT OR REPLACE INTO records (cksum, status) VALUES (?, ?)",
                (digest, PRESENT),
            )
    except (sqlite3.Error, OSError) as error:
        local_db.close()
        unreachable(
            "Durability anytime: restoring the local ledger failed",
            {"error": str(error), "cksum": digest},
        )
        return False
    local_db.close()

    if database_error is not None:
        reachable(
            "Durability anytime: database validation can be unavailable",
            {"error": str(database_error), "cksum": digest},
        )
        return False

    always(
        exists,
        "Durability: any present value must be in the database",
        {"cksum": digest, "exists": exists},
    )
    return True


def validate_all(db: Database) -> bool:
    """
    Check that all keys we inserted are present in the database. Used as part of a finally_ driver.
    """
    local_db = open_sqlite()
    try:
        present = {
            row[0]
            for row in local_db.execute(
                "SELECT cksum FROM records WHERE status = ?", (PRESENT,)
            )
        }
    except (sqlite3.Error, OSError) as error:
        local_db.close()
        unreachable(
            "Durability final: reading the local ledger failed",
            {"error": str(error)},
        )
        return False
    local_db.close()

    # Note: this test can only be run while no read/write requests are in flight to the database.
    found: set[str] = set()
    try:
        checksums = list(present)
        with db.connect() as connection:
            for start in range(0, len(checksums), BATCH_SIZE):
                batch = checksums[start : start + BATCH_SIZE]
                found.update(
                    text(row[0])
                    for row in connection.execute(
                        'SELECT cksum FROM "values" WHERE cksum = ANY(%s)',
                        (batch,),
                    )
                )
    except (psycopg.Error, OSError) as error:
        reachable(
            "Durability final: database validation can be unavailable",
            {"error": str(error)},
        )
        return False

    missing = sorted(present - found)
    always(
        not missing,
        "Durability final: any present value must be in the database",
        {"missing": missing[:20], "missing_count": len(missing)},
    )
    return True
