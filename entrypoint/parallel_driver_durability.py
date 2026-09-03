#!/usr/bin/env python3
from antithesis.random import random_choice

from durability.delete import run_once as delete_once
from durability.insert import run_once as insert_once
from test_client.database import Database
from test_client.repeat import repeat


if __name__ == "__main__":
    database = Database()
    repeat(lambda: random_choice([insert_once, delete_once])(database))
