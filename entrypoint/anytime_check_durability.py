#!/usr/bin/env python3
from importlib import import_module

from test_client.database import Database
from test_client.repeat import repeat


if __name__ == "__main__":
    validate_one = import_module("durability.validate").validate_one
    database = Database()
    repeat(lambda: validate_one(database))
