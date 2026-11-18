"""Exec a recovery subprocess with kernel-enforced owner-death cleanup (Linux)."""

import ctypes
import os
import signal
import sys


if __name__ == "__main__":
    # Run after exec, never Python preexec_fn inside the threaded API process.
    parent = int(sys.argv[1])
    native = ctypes.CDLL(None, use_errno=True).prctl
    native.argtypes = [ctypes.c_int] + [ctypes.c_ulong] * 4
    native.restype = ctypes.c_int
    if native(1, signal.SIGKILL, 0, 0, 0) != 0 or os.getppid() != parent:
        sys.exit(1)
    os.execv(sys.argv[2], sys.argv[2:])
