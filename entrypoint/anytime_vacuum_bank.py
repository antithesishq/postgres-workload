#!/usr/bin/env python3
from bank.vacuum import run_once
from test_client.database import Database


if __name__ == "__main__":
    run_once(Database())
