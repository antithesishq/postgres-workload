#!/usr/bin/env python3
from importlib import import_module
import time

from antithesis.assertions import always

from test_client.database import Database

RECOVERY_SECONDS = 180


def main() -> None:
    bank_validate = import_module("bank.validate").validate
    durability_validate_all = import_module("durability.validate").validate_all
    database = Database()
    deadline = time.monotonic() + RECOVERY_SECONDS
    time.sleep(5)
    online = False
    while time.monotonic() < deadline:
        if database.ping():
            online = True
            break
        time.sleep(1)

    always(
        online,
        "Final validation: database recovers within 180 seconds",
        {"timeout_seconds": RECOVERY_SECONDS},
    )
    if not online:
        return

    bank_validated = False
    durability_validated = False
    while time.monotonic() < deadline and not (bank_validated and durability_validated):
        if not bank_validated:
            bank_validated = bank_validate(database)
        if not durability_validated:
            durability_validated = durability_validate_all(database)
        if not (bank_validated and durability_validated):
            time.sleep(1)

    always(
        bank_validated and durability_validated,
        "Final validation: both workload validators complete after recovery",
        {
            "bank_validated": bank_validated,
            "durability_validated": durability_validated,
        },
    )


if __name__ == "__main__":
    main()
