from antithesis.assertions import reachable
from antithesis.random import random_choice
import psycopg

from test_client.database import Database


def run_once(db: Database) -> bool:
    mode = random_choice(["ordinary", "freeze", "full"])
    statement = {
        "ordinary": 'VACUUM public."values"',
        "freeze": 'VACUUM (FREEZE) public."values"',
        "full": 'VACUUM (FULL) public."values"',
    }[mode]
    try:
        with db.connect() as connection:
            connection.autocommit = True
            connection.execute(statement)
    except (psycopg.Error, OSError) as error:
        print(f"durability vacuum ({mode}) was interrupted: {error}", flush=True)
        return False
    if mode == "freeze":
        reachable("Durability: vacuum freeze completed")
    elif mode == "full":
        reachable("Durability: vacuum full completed")
    else:
        reachable("Durability: vacuum completed")
    return True
