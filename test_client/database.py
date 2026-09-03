from __future__ import annotations

import os
from contextlib import contextmanager
from typing import Iterator

import psycopg


class Database:
    def __init__(self, url: str | None = None) -> None:
        self.url = url or os.environ["DATABASE_URL"]

    @contextmanager
    def connect(self) -> Iterator[psycopg.Connection]:
        with psycopg.connect(
            self.url,
            connect_timeout=3,
            keepalives=1,
            keepalives_idle=3,
            keepalives_interval=3,
            keepalives_count=3,
            tcp_user_timeout=3000,
            options="-c statement_timeout=30000 -c lock_timeout=3000",
        ) as connection:
            yield connection

    def ping(self) -> bool:
        try:
            with self.connect() as connection:
                connection.execute("SELECT 1")
            return True
        except psycopg.Error as error:
            print(f"database ping failed: {error}", flush=True)
            return False
