#!/usr/bin/env python3
from test_client.database import Database
from test_client.switch_wal import run_once


if __name__ == "__main__":
    run_once(Database())
