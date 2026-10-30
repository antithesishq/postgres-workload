from __future__ import annotations

from collections.abc import Callable

from antithesis.random import get_random, random_choice
import psycopg

TOTAL_BALANCE = 1_000_000
MIN_INITIAL_ACCOUNTS = 2
MAX_INITIAL_ACCOUNTS = 100
TRANSACTION_RETRIES = 5
RETRYABLE_SQLSTATES = {"40001", "40P01"}
PADDING_LENGTHS = (0, 1, 255, 1023, 2047, 2048, 2049, 8192, 65536)


def random_text(length: int, draw: Callable[[], int] = get_random) -> str:
    chunks: list[str] = []
    remaining = length
    while remaining:
        chunk = f"{draw() & 0xFFFFFFFFFFFFFFFF:016x}"
        chunks.append(chunk[:remaining])
        remaining -= min(remaining, len(chunk))
    return "".join(chunks)


def random_padding() -> str:
    return random_text(random_choice(list(PADDING_LENGTHS)))


def distribute_balance(
    total: int, count: int, draw: Callable[[], int] = get_random
) -> list[int]:
    if count < 1:
        raise ValueError("count must be positive")
    cuts = sorted(draw() % (total + 1) for _ in range(count - 1))
    points = [0, *cuts, total]
    return [right - left for left, right in zip(points, points[1:])]


def begin_serializable(connection: psycopg.Connection) -> None:
    connection.execute("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")


def account_ids(connection: psycopg.Connection) -> list[int]:
    return [row[0] for row in connection.execute("SELECT id FROM accounts ORDER BY id")]


def sample_pair(connection: psycopg.Connection) -> tuple[int, int] | None:
    """
    Pick two distinct account ids with a plain READ COMMITTED read, committed on
    its own before the caller opens its SERIALIZABLE transaction.

    Reading the whole table inside a SERIALIZABLE transaction takes a
    relation-level predicate lock, so every concurrent insert, update or delete
    on `accounts` becomes a read/write dependency on the sampler and the
    transaction is aborted with 40001 far more often than its two rows deserve.
    Sampling outside keeps the serializable read set to the two chosen rows.

    The sample may be stale: a chosen account can be deleted before the caller
    re-reads it. The caller's own SELECT inside its transaction is the existence
    check, and finding fewer than two rows there is a normal outcome.
    """
    ids = account_ids(connection)
    connection.commit()
    return choose_two(ids)


def choose_two(ids: list[int]) -> tuple[int, int] | None:
    if len(ids) < 2:
        return None
    first_index = get_random() % len(ids)
    second_index = get_random() % (len(ids) - 1)
    if second_index >= first_index:
        second_index += 1
    return ids[first_index], ids[second_index]


def truncate_length(length: int) -> int:
    if length <= 0:
        return 0
    candidates = sorted({0, min(1, length), length // 2, max(0, length - 1)})
    return candidates[get_random() % len(candidates)]
