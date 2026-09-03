#!/usr/bin/env python3
from bank.setup import setup as setup_bank
from durability.setup import setup as setup_durability
from test_client.database import Database


if __name__ == "__main__":
    database = Database()
    setup_bank(database)
    setup_durability(database)
