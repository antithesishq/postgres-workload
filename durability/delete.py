from sre_parse import OP_IGNORE

from antithesis.assertions import reachable, unreachable
from antithesis.random import get_random
import psycopg
import sqlite3

from durability.helper import NOT_PRESENT, UNKNOWN, open_sqlite
from test_client.database import Database


def run_once(db: Database) -> bool:
    # First, get a random record from the local record of entries we have previously
    # inserted (or tried to insert) into the database.
    local_db = open_sqlite()
    try:
        with local_db:
            local_db.execute("BEGIN IMMEDIATE")
            count = local_db.execute("SELECT COUNT(*) FROM records").fetchone()[0]
            digest = None
            if count:
                offset = get_random() % count
                digest = local_db.execute(
                    "SELECT cksum FROM records ORDER BY cksum LIMIT 1 OFFSET ?",
                    (offset,),
                ).fetchone()[0]
                local_db.execute("DELETE FROM records WHERE cksum = ?", (digest,))
    except (sqlite3.Error, OSError) as error:
        local_db.close()
        unreachable(
            "Durability delete: removing a value from the local ledger failed",
            {"error": str(error)},
        )
        return False

    if digest is None:
        local_db.close()
        return False

    # Then, carry out a delete request against the database.
    try:
        with db.connect() as connection:
            connection.execute('DELETE FROM "values" WHERE cksum = %s', (digest,))
        # If the delete suceeded, we can mark this as not present.
        status = NOT_PRESENT
    except (psycopg.Error, OSError) as error:
        status = UNKNOWN
        reachable(
            "Durability delete: database outcome is unknown",
            {"error": str(error), "cksum": digest},
        )

    # now update the status in our local record of values in the database
    try:
        with local_db:
            local_db.execute(
                "INSERT OR REPLACE INTO records (cksum, status) VALUES (?, ?)",
                (digest, status),
            )
    except (sqlite3.Error, OSError) as error:
        local_db.close()
        unreachable(
            "Durability delete: recording the local outcome failed",
            {"error": str(error), "cksum": digest, "status": status},
        )
        return False
    local_db.close()

    if status == NOT_PRESENT:
        reachable(
            "Durability: successful delete outcome recorded locally",
            {"cksum": digest},
        )
    return status != UNKNOWN
