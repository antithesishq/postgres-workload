#!/usr/bin/env python3
from importlib import import_module

from antithesis.random import get_random

from test_client.database import Database
from test_client.repeat import repeat


if __name__ == "__main__":
    validate = import_module("bank.validate").validate
    database = Database()
    repeat(
        lambda: validate(database, use_read_cursor=get_random() % 2 == 0)
    )
