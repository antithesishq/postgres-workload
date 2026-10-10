from antithesis.assertions import always, reachable, unreachable
from antithesis.random import get_random
import psycopg
import sqlite3

from durability.helper import BATCH_SIZE, PRESENT, checksum, open_sqlite, text
from test_client.database import Database


def validate_one(db: Database) -> bool:
    """
    Pick a random present key and check its presence and payload integrity.

    We remove the key from the local ledger to prevent other drivers from
    deleting it in the meantime.
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

    # Fetch the payload so we can check it against the original digest.
    database_error = None
    try:
        with db.connect() as connection:
            row = connection.execute(
                'SELECT data FROM "values" WHERE cksum = %s',
                (digest,),
            ).fetchone()
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

    exists = row is not None
    always(
        exists,
        "Durability: any present value must be in the database",
        {"cksum": digest, "exists": exists},
    )
    if exists:
        actual_digest = checksum(row[0])
        always(
            actual_digest == digest,
            "Durability: any present value must match its original checksum",
            {"cksum": digest, "actual_cksum": actual_digest},
        )
    return True


def validate_all(db: Database) -> bool:
    """
    Check presence and payload integrity for all present keys in a finally_ driver.
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
    corrupted: list[dict[str, str]] = []
    corrupted_count = 0
    try:
        checksums = list(present)
        with db.connect() as connection:
            for start in range(0, len(checksums), BATCH_SIZE):
                batch = checksums[start : start + BATCH_SIZE]
                for digest, data in connection.execute(
                    'SELECT cksum, data FROM "values" WHERE cksum = ANY(%s)',
                    (batch,),
                ):
                    digest = text(digest)
                    found.add(digest)
                    actual_digest = checksum(data)
                    if actual_digest != digest:
                        corrupted_count += 1
                        if len(corrupted) < 20:
                            corrupted.append(
                                {"cksum": digest, "actual_cksum": actual_digest}
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
    always(
        corrupted_count == 0,
        "Durability final: any present value must match its original checksum",
        {"corrupted": corrupted, "corrupted_count": corrupted_count},
    )
    return True
