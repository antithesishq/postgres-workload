from __future__ import annotations

import hashlib
import os
from pathlib import Path
import sqlite3

from antithesis.random import get_random

DEFAULT_LEDGER_PATH = "/var/lib/antigres/durability.sqlite3"
BATCH_SIZE = 500
MIN_DATA_LENGTH = 256
MAX_DATA_LENGTH = 16 * 1024
UNKNOWN = "unknown"
PRESENT = "present"
NOT_PRESENT = "not_present"


def random_text(length: int) -> str:
    chunks: list[str] = []
    remaining = length
    while remaining:
        chunk = f"{get_random() & 0xFFFFFFFFFFFFFFFF:016x}"
        chunks.append(chunk[:remaining])
        remaining -= min(remaining, len(chunk))
    return "".join(chunks)


def random_data() -> str:
    length = get_random() % (MAX_DATA_LENGTH - MIN_DATA_LENGTH + 1) + MIN_DATA_LENGTH
    return random_text(length)


def checksum(data: str | bytes) -> str:
    if isinstance(data, str):
        data = data.encode("utf-8")
    return hashlib.sha256(data).hexdigest()


def text(value: str | bytes) -> str:
    return value.decode("utf-8") if isinstance(value, bytes) else value


def ledger_path() -> Path:
    return Path(os.environ.get("DURABILITY_LEDGER_PATH", DEFAULT_LEDGER_PATH))


def open_sqlite(*, initialize: bool = False) -> sqlite3.Connection:
    path = ledger_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    connection = sqlite3.connect(path, timeout=10)
    connection.execute("PRAGMA busy_timeout=10000")
    connection.execute("PRAGMA synchronous=FULL")
    if initialize:
        connection.execute("PRAGMA journal_mode=WAL")
        connection.execute(
            """
            CREATE TABLE IF NOT EXISTS records (
                cksum TEXT PRIMARY KEY NOT NULL,
                status TEXT NOT NULL CHECK (
                    status IN ('unknown', 'present', 'not_present')
                )
            )
            """
        )
        connection.execute(
            "CREATE INDEX IF NOT EXISTS records_status ON records (status)"
        )
    return connection
