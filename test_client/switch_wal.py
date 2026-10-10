from antithesis.assertions import reachable
import psycopg

from test_client.database import Database


def run_once(db: Database) -> bool:
    try:
        with db.connect() as connection:
            connection.autocommit = True
            connection.execute("SELECT pg_catalog.pg_switch_wal()").fetchone()
    except (psycopg.Error, OSError) as error:
        print(f"WAL switch was interrupted: {error}", flush=True)
        return False
    reachable("WAL: switch request completed")
    return True
