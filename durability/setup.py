from durability.helper import open_sqlite
from test_client.database import Database


def setup(db: Database) -> None:
    with db.connect() as connection:
        connection.execute(
            'CREATE TABLE IF NOT EXISTS "values" ('
            "data TEXT NOT NULL, "
            "cksum TEXT PRIMARY KEY NOT NULL)"
        )
    local_db = open_sqlite(initialize=True)
    with local_db:
        local_db.execute("SELECT 1")
    local_db.close()
