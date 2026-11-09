"""Проверка: запасные соединения браузера не блокируют запросы UI.

Запустить UI, затем: python tools/test_ui_connections.py [порт].
"""

import concurrent.futures
import socket
import sys
import time
import urllib.request


def main():
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8791
    idle = []
    try:
        for _ in range(4):
            idle.append(socket.create_connection(("127.0.0.1", port), timeout=3))

        def fetch(_):
            start = time.perf_counter()
            with urllib.request.urlopen(
                f"http://127.0.0.1:{port}/", timeout=3
            ) as response:
                assert response.status == 200
                assert b"searchForm" in response.read()
            return round(time.perf_counter() - start, 3)

        with concurrent.futures.ThreadPoolExecutor(max_workers=6) as pool:
            print("OK:", list(pool.map(fetch, range(6))))
    finally:
        for connection in idle:
            connection.close()


if __name__ == "__main__":
    main()
