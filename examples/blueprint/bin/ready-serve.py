#!/usr/bin/env python3
"""Tiny TCP readiness stub for `jhol dev --spawn` demos (no deps)."""
import socket
import sys
import time

host, port = "127.0.0.1", int(sys.argv[1]) if len(sys.argv) > 1 else 18080
srv = socket.socket()
srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
srv.bind((host, port))
srv.listen(8)
# Keep accepting until killed by the scheduler Drop cleanup.
while True:
    try:
        conn, _ = srv.accept()
        conn.close()
    except Exception:
        time.sleep(0.05)
