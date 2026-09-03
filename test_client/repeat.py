from collections.abc import Callable

from antithesis.random import get_random


def repeat(action: Callable[[], object]) -> None:
    stop_numerator = get_random() % 200 + 1
    while True:
        action()
        if get_random() % 1000 < stop_numerator:
            return
