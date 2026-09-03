from antithesis.assertions import always
import psycopg

from bank.helper import TOTAL_BALANCE
from test_client.database import Database

BATCH_SIZE = 500


def _read_with_cursor(connection: psycopg.Connection) -> tuple[int | None, int | None, int]:
    total = 0
    minimum: int | None = None
    count = 0
    with connection.cursor(name="bank_validation") as cursor:
        cursor.execute("SELECT balance FROM accounts ORDER BY id")
        while rows := cursor.fetchmany(BATCH_SIZE):
            for (balance,) in rows:
                total += balance
                minimum = balance if minimum is None else min(minimum, balance)
                count += 1
    return (total if count else None), minimum, count


def validate(db: Database, *, use_read_cursor: bool = False) -> bool:
    try:
        with db.connect() as connection:
            if use_read_cursor:
                total, minimum, count = _read_with_cursor(connection)
            else:
                total, minimum, count = connection.execute(
                    """
                    SELECT SUM(balance),
                           MIN(balance),
                           COUNT(*)
                    FROM accounts
                    """
                ).fetchone()
            if total is not None:
                total = int(total)
            if minimum is not None:
                minimum = int(minimum)
    except (psycopg.Error, OSError) as error:
        print(f"bank validation unavailable: {error}", flush=True)
        return False

    always(
        count >= 1,
        "Bank: at least one account always exists",
        {"accounts": count, "read_cursor": use_read_cursor},
    )
    always(
        total == TOTAL_BALANCE,
        "Bank: total balance is always one million",
        {
            "actual": total,
            "expected": TOTAL_BALANCE,
            "accounts": count,
            "read_cursor": use_read_cursor,
        },
    )
    always(
        minimum is not None and minimum >= 0,
        "Bank: account balances are always non-negative",
        {
            "minimum_balance": minimum,
            "accounts": count,
            "read_cursor": use_read_cursor,
        },
    )
    return True
