from antithesis.assertions import reachable
from antithesis.random import random_choice
import psycopg

from test_client.database import Database


def run_once(db: Database) -> bool:
    mode = random_choice(["ordinary", "freeze", "full"])
    statement = {
        "ordinary": 'VACUUM public.accounts',
        "freeze": 'VACUUM (FREEZE) public.accounts',
        "full": 'VACUUM (FULL) public.accounts',
    }[mode]
    try:
        with db.connect() as connection:
            connection.autocommit = True
            connection.execute(statement)
    except (psycopg.Error, OSError) as error:
        print(f"bank vacuum ({mode}) was interrupted: {error}", flush=True)
        return False
    if mode == "freeze":
        reachable("Bank: vacuum freeze completed")
    elif mode == "full":
        reachable("Bank: vacuum full completed")
    else:
        reachable("Bank: vacuum completed")
    return True
