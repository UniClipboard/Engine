#!/usr/bin/env python3
"""Detect discontinuous realtime changes across one external E2E capture."""
import ctypes
import errno
import json
import os
import sys
import time


class Timespec(ctypes.Structure):
    _fields_ = [('seconds', ctypes.c_long), ('nanoseconds', ctypes.c_long)]


class TimerSpec(ctypes.Structure):
    _fields_ = [('interval', Timespec), ('value', Timespec)]


def sample():
    before = time.time_ns()
    monotonic = time.monotonic_ns()
    after = time.time_ns()
    return {'realtime_before_ns': str(before), 'monotonic_ns': str(monotonic),
            'realtime_after_ns': str(after)}


libc = ctypes.CDLL(None, use_errno=True)
libc.timerfd_create.argtypes = [ctypes.c_int, ctypes.c_int]
libc.timerfd_settime.argtypes = [ctypes.c_int, ctypes.c_int,
                                  ctypes.POINTER(TimerSpec), ctypes.c_void_p]
fd = libc.timerfd_create(time.CLOCK_REALTIME, os.O_NONBLOCK | os.O_CLOEXEC)
if fd < 0:
    raise OSError(ctypes.get_errno(), 'timerfd_create')
try:
    deadline = int(time.time()) + 86400
    spec = TimerSpec(Timespec(0, 0), Timespec(deadline, 0))
    # Linux 原生取消标志覆盖整个区间，不用采样间隔推断没有时钟跳变。
    if libc.timerfd_settime(fd, 1 | 2, ctypes.byref(spec), None) != 0:
        raise OSError(ctypes.get_errno(), 'timerfd_settime')
    first = sample()
    print(json.dumps({'ready': True, 'sample': first}), flush=True)
    sys.stdin.readline()
    last = sample()
    outcome = 'unexpected_expiration'
    try:
        os.read(fd, 8)
    except OSError as error:
        outcome = ('no_discontinuous_change' if error.errno == errno.EAGAIN
                   else 'discontinuous_change' if error.errno == errno.ECANCELED
                   else 'read_failed')
    print(json.dumps({'outcome': outcome, 'sample': last}), flush=True)
finally:
    os.close(fd)
