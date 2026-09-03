#!/usr/bin/env python3
from bank.account_operations import run_once
from test_client.database import Database
from test_client.repeat import repeat


if __name__ == "__main__":
    database = Database()
    repeat(lambda: run_once(database))
