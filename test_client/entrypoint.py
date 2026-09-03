import signal
import time

from antithesis.lifecycle import setup_complete

from test_client.database import Database


def main() -> None:
    database = Database()
    while not database.ping():
        time.sleep(1)

    setup_complete({"database": "ready"})
    print("test client setup complete", flush=True)
    while True:
        signal.pause()


if __name__ == "__main__":
    main()
