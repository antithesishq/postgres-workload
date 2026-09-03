from antithesis.assertions import reachable, unreachable
import psycopg
import sqlite3

from durability.helper import (
    PRESENT,
    UNKNOWN,
    checksum,
    open_sqlite,
    random_data,
)
from test_client.database import Database


def run_once(db: Database) -> bool:
    data = random_data()
    digest = checksum(data)

    try:
        with db.connect() as connection:
            connection.execute(
                'INSERT INTO "values" (data, cksum) VALUES (%s, %s)',
                (data, digest),
            )
        status = PRESENT
    except psycopg.errors.UniqueViolation:
        status = PRESENT
        reachable(
            "Durability insert: existing key confirmed present",
            {"cksum": digest},
        )
    except (psycopg.Error, OSError) as error:
        status = UNKNOWN
        reachable(
            "Durability insert: database outcome is unknown",
            {"error": str(error), "cksum": digest},
        )

    local_db = open_sqlite()
    try:
        with local_db:
            local_db.execute(
                "INSERT OR REPLACE INTO records (cksum, status) VALUES (?, ?)",
                (digest, status),
            )
    except (sqlite3.Error, OSError) as error:
        local_db.close()
        unreachable(
            "Durability insert: recording the local outcome failed",
            {"error": str(error), "cksum": digest, "status": status},
        )
        return False
    local_db.close()

    if status == PRESENT:
        reachable(
            "Durability: key confirmed present",
            {"cksum": digest, "data_bytes": len(data)},
        )
    return status != UNKNOWN
