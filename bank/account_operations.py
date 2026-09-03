from antithesis.assertions import reachable
from antithesis.random import get_random, random_choice
import psycopg

from bank.helper import (
    RETRYABLE_SQLSTATES,
    TRANSACTION_RETRIES,
    account_ids,
    begin_serializable,
    choose_two,
    random_padding,
    truncate_length,
)
from test_client.database import Database


def transfer(db: Database) -> bool:
    with db.connect() as connection:
        with connection.transaction():
            begin_serializable(connection)
            pair = choose_two(account_ids(connection))
            if pair is None:
                return False
            sender_id, recipient_id = pair
            rows = connection.execute(
                "SELECT id, balance FROM accounts WHERE id = ANY(%s) ORDER BY id FOR UPDATE",
                (list(pair),),
            ).fetchall()
            if len(rows) != 2:
                return False
            sender_balance = {row[0]: row[1] for row in rows}[sender_id]
            if sender_balance == 0:
                return False
            amount = get_random() % sender_balance + 1
            connection.execute(
                "UPDATE accounts SET balance = balance - %s WHERE id = %s",
                (amount, sender_id),
            )
            connection.execute(
                "UPDATE accounts SET balance = balance + %s WHERE id = %s",
                (amount, recipient_id),
            )
    reachable(
        "Bank: transfer committed",
        {"sender": sender_id, "recipient": recipient_id, "amount": amount},
    )
    return True


def add_account(db: Database) -> bool:
    padding = random_padding()
    with db.connect() as connection:
        account_id = connection.execute(
            "INSERT INTO accounts (balance, padding) VALUES (0, %s) RETURNING id",
            (padding,),
        ).fetchone()[0]
    reachable(
        "Bank: zero-balance account added",
        {"account": account_id, "padding_bytes": len(padding)},
    )
    return True


def delete_account(db: Database) -> bool:
    with db.connect() as connection:
        with connection.transaction():
            begin_serializable(connection)
            pair = choose_two(account_ids(connection))
            if pair is None:
                return False
            source_id, recipient_id = pair
            rows = connection.execute(
                "SELECT id, balance FROM accounts WHERE id = ANY(%s) ORDER BY id FOR UPDATE",
                (list(pair),),
            ).fetchall()
            if len(rows) != 2:
                return False
            source_balance = {row[0]: row[1] for row in rows}[source_id]
            connection.execute(
                "UPDATE accounts SET balance = balance + %s WHERE id = %s",
                (source_balance, recipient_id),
            )
            connection.execute("DELETE FROM accounts WHERE id = %s", (source_id,))
    reachable(
        "Bank: account merged and deleted",
        {"source": source_id, "recipient": recipient_id, "balance": source_balance},
    )
    return True


def change_padding(db: Database, operation: str) -> bool:
    with db.connect() as connection:
        with connection.transaction():
            ids = account_ids(connection)
            if not ids:
                return False
            account_id = ids[get_random() % len(ids)]
            row = connection.execute(
                "SELECT padding FROM accounts WHERE id = %s FOR UPDATE", (account_id,)
            ).fetchone()
            if row is None:
                return False
            old_padding = row[0]
            if operation == "null":
                new_padding = None
            elif operation == "truncate":
                if old_padding is None:
                    return False
                new_padding = old_padding[: truncate_length(len(old_padding))]
            else:
                new_padding = random_padding()
            connection.execute(
                "UPDATE accounts SET padding = %s WHERE id = %s",
                (new_padding, account_id),
            )

    if operation == "null":
        reachable(
            "Bank: account padding cleared to null",
            {"account": account_id, "previous_bytes": len(old_padding or "")},
        )
    elif operation == "truncate":
        reachable(
            "Bank: account padding truncated",
            {"account": account_id, "new_bytes": len(new_padding)},
        )
    elif old_padding is None:
        reachable(
            "Bank: null account padding restored",
            {"account": account_id, "new_bytes": len(new_padding)},
        )
    else:
        reachable(
            "Bank: account padding replaced",
            {"account": account_id, "new_bytes": len(new_padding)},
        )
    return True


def run_once(db: Database) -> bool:
    operation = random_choice(
        [
            "transfer",
            "add",
            "delete",
            "padding_null",
            "padding_truncate",
            "padding_replace",
        ]
    )
    for attempt in range(1, TRANSACTION_RETRIES + 1):
        try:
            if operation == "transfer":
                return transfer(db)
            if operation == "add":
                return add_account(db)
            if operation == "delete":
                return delete_account(db)
            return change_padding(db, operation.removeprefix("padding_"))
        except psycopg.Error as error:
            if error.sqlstate in RETRYABLE_SQLSTATES and attempt < TRANSACTION_RETRIES:
                continue
            print(f"bank {operation} was interrupted: {error}", flush=True)
            return False
        except OSError as error:
            print(f"bank {operation} was interrupted: {error}", flush=True)
            return False
    return False
