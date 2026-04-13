#!/usr/bin/env python3
"""Cross-validate NativeClient vs picows against THREE neutral servers.

Rationale (following @tarasko's issue #11 guidance): a single server can
favour one client architecture. Run both clients against three independent
pure-Rust server implementations and report each matchup, so the winner is
visible across server choices — not a function of one specific echo server.

Servers:
  1. tokio-tungstenite (ours, benches/ws_echo_server.rs)
  2. fastwebsockets (Deno/Cloudflare, benches/ws_echo_fastws.rs)
  3. picows (Cython, run via its own asyncio.Protocol echo in-subprocess)
"""

import asyncio
import multiprocessing as mp
import os
import signal
import statistics
import struct
import subprocess
import time

import uvloop

uvloop.install()

from websocket_rs.native_client import connect as native_connect

try:
    from picows import WSCloseCode, WSFrame, WSListener, WSMsgType, WSTransport, ws_connect, ws_create_server

    PICOWS = True
except ImportError:
    PICOWS = False

WINDOW = 100
WARMUP = 200
N = 5000


# ---------- Servers ----------


def start_rust_server(bin_name: str, port: int) -> subprocess.Popen:
    path = f"target/release/{bin_name}"
    if not os.path.exists(path):
        raise RuntimeError(f"{path} not built")
    import shutil

    # Pin server to core 0 (client process pins itself to core 1 below) — same
    # CCD on Ryzen 9950X for L3 sharing while avoiding contention. Falls back
    # to no-pinning if `taskset` isn't available (macOS/Windows/minimal CI).
    cmd = [path, str(port)]
    if shutil.which("taskset"):
        cmd = ["taskset", "-c", "0", *cmd]
    p = subprocess.Popen(
        cmd,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    import socket as _s

    for _ in range(50):
        try:
            with _s.create_connection(("127.0.0.1", port), timeout=0.1):
                return p
        except OSError:
            time.sleep(0.1)
    p.terminate()
    raise RuntimeError(f"{bin_name} didn't open port")


def _picows_echo_proc(port: int, ready):
    import asyncio
    import os

    import uvloop

    # Pin server to core 0 (matches Rust echo binaries).
    try:
        os.sched_setaffinity(0, {0})
    except Exception:
        pass
    uvloop.install()
    from picows import WSCloseCode, WSFrame, WSListener, WSMsgType, WSTransport, ws_create_server

    class EchoL(WSListener):
        def on_ws_frame(self, transport: WSTransport, frame: WSFrame):
            if frame.msg_type == WSMsgType.CLOSE:
                transport.send_close(WSCloseCode.OK)
                transport.disconnect()
                return
            if frame.msg_type in (WSMsgType.BINARY, WSMsgType.TEXT):
                mv = frame.get_payload_as_memoryview()
                n = len(mv)
                mid = struct.unpack_from("=I", mv, 0)[0] if n >= 4 else 0
                out = bytearray(24 + n)
                struct.pack_into("=IddI", out, 0, mid, 0.0, 0.0, n)
                out[24:] = mv
                transport.send(WSMsgType.BINARY, out)

    async def main():
        await ws_create_server(EchoL, "127.0.0.1", port)
        ready.set()
        await asyncio.Future()

    asyncio.run(main())


def start_picows_server(port: int):
    ctx = mp.get_context("spawn")
    ready = ctx.Event()
    p = ctx.Process(target=_picows_echo_proc, args=(port, ready), daemon=True)
    p.start()
    ready.wait(timeout=10)
    time.sleep(0.3)
    return p


# ---------- Clients ----------


def parse_mid(resp):
    return struct.unpack_from("=I", resp, 0)[0]


async def native_cb_pipelined(uri, size, n):
    """Native client using on_message callback — picows-style architecture.

    Same as native_pipelined but delivers messages via a sync callback
    instead of awaiting futures. Exists to isolate architectural (await vs
    callback) cost from implementation cost when comparing against picows.
    """
    base = b"a" * size
    loop = asyncio.get_running_loop()
    done = loop.create_future()
    send_times = {}
    rtts = []
    ws_holder = [None]
    sent = received = w_done = 0
    in_warmup = True

    def on_msg(msg):
        nonlocal sent, received, w_done, in_warmup
        if in_warmup:
            w_done += 1
            if w_done >= WARMUP:
                in_warmup = False
                _fill()
            return
        t_recv = time.perf_counter()
        mid = struct.unpack_from("=I", memoryview(msg))[0]
        rtts.append((t_recv - send_times.pop(mid)) * 1000)
        received += 1
        if received >= n:
            if not done.done():
                done.set_result(None)
            return
        _fill()

    def _fill():
        nonlocal sent
        ws = ws_holder[0]
        while sent < n and (sent - received) < WINDOW:
            send_times[sent] = time.perf_counter()
            ws.send(struct.pack("=I", sent) + base)
            sent += 1

    ws = await native_connect(uri, on_message=on_msg)
    ws_holder[0] = ws
    for _ in range(WARMUP):
        ws.send(struct.pack("=I", 9999) + b"w")
    await done
    ws.close()
    return statistics.mean(rtts), statistics.median(rtts), statistics.quantiles(rtts, n=100)[98]


async def native_rr(uri, size, n):
    base = b"a" * size
    ws = await native_connect(uri)
    for _ in range(WARMUP):
        ws.send(struct.pack("=I", 9999) + b"w")
        await ws.recv()
    rtts = []
    for mid in range(n):
        t0 = time.perf_counter()
        ws.send(struct.pack("=I", mid) + base)
        await ws.recv()
        rtts.append((time.perf_counter() - t0) * 1000)
    ws.close()
    return statistics.mean(rtts), statistics.median(rtts), statistics.quantiles(rtts, n=100)[98]


async def native_pipelined(uri, size, n):
    base = b"a" * size
    ws = await native_connect(uri)
    for _ in range(WARMUP):
        ws.send(struct.pack("=I", 9999) + b"w")
        await ws.recv()
    send_times = {}
    rtts = []
    sent = recv = 0
    while recv < n:
        while sent < n and (sent - recv) < WINDOW:
            send_times[sent] = time.perf_counter()
            ws.send(struct.pack("=I", sent) + base)
            sent += 1
        resp = await ws.recv()
        t_recv = time.perf_counter()
        recv += 1
        rtts.append((t_recv - send_times[parse_mid(resp)]) * 1000)
    ws.close()
    return statistics.mean(rtts), statistics.median(rtts), statistics.quantiles(rtts, n=100)[98]


async def picows_rr(uri, size, n):
    base_msg = bytearray(b"a") * (size + 4)
    mv = memoryview(base_msg)
    loop = asyncio.get_running_loop()
    st = {"fut": None}

    class L(WSListener):
        def on_ws_frame(self, t, f: WSFrame):
            fut = st["fut"]
            if fut and not fut.done():
                fut.set_result(None)

    transport, _ = await ws_connect(L, uri)
    for _ in range(WARMUP):
        mv[:4] = struct.pack("=I", 9999)
        st["fut"] = loop.create_future()
        transport.send(WSMsgType.BINARY, base_msg)
        await st["fut"]
    rtts = []
    for mid in range(n):
        mv[:4] = struct.pack("=I", mid)
        st["fut"] = loop.create_future()
        t0 = time.perf_counter()
        transport.send(WSMsgType.BINARY, base_msg)
        await st["fut"]
        rtts.append((time.perf_counter() - t0) * 1000)
    transport.disconnect()
    return statistics.mean(rtts), statistics.median(rtts), statistics.quantiles(rtts, n=100)[98]


async def picows_pipelined(uri, size, n):
    base_msg = bytearray(b"a") * (size + 4)
    mv = memoryview(base_msg)
    loop = asyncio.get_running_loop()
    done = loop.create_future()
    send_times = {}
    rtts = []

    class L(WSListener):
        def __init__(self):
            self.transport = None
            self.sent = 0
            self.received = 0
            self.in_warmup = True
            self.w_done = 0

        def on_ws_connected(self, t):
            self.transport = t
            for _ in range(WARMUP):
                mv[:4] = struct.pack("=I", 9999)
                t.send(WSMsgType.BINARY, base_msg)

        def on_ws_frame(self, t, f: WSFrame):
            if self.in_warmup:
                self.w_done += 1
                if self.w_done >= WARMUP:
                    self.in_warmup = False
                    self._fill()
                return
            t_recv = time.perf_counter()
            payload = bytes(f.get_payload_as_memoryview())
            rtts.append((t_recv - send_times[parse_mid(payload)]) * 1000)
            self.received += 1
            if self.received >= n:
                t.disconnect()
                if not done.done():
                    done.set_result(None)
                return
            self._fill()

        def _fill(self):
            while self.sent < n and (self.sent - self.received) < WINDOW:
                mv[:4] = struct.pack("=I", self.sent)
                send_times[self.sent] = time.perf_counter()
                self.transport.send(WSMsgType.BINARY, base_msg)
                self.sent += 1

    transport, _ = await ws_connect(L, uri)
    await done
    return statistics.mean(rtts), statistics.median(rtts), statistics.quantiles(rtts, n=100)[98]


# ---------- Main ----------


async def run_against(server_label: str, port: int):
    uri = f"ws://127.0.0.1:{port}"
    print(f"\n=== Server: {server_label} ===")
    print(f"{'size':>6} {'mode':>12} {'native(await)':>22} {'native(on_message)':>22} {'picows':>22}")
    print(f"{'':6} {'':12} {'mean/p50/p99 ms':>22} {'mean/p50/p99 ms':>22} {'mean/p50/p99 ms':>22}")
    print("-" * 94)
    for size in (512, 4096, 16384, 65536):
        for mode in ("RR", "pipelined"):
            fn_n = native_rr if mode == "RR" else native_pipelined
            fn_p = picows_rr if mode == "RR" else picows_pipelined
            n_m, n_p50, n_p99 = await fn_n(uri, size, N)
            # Callback mode only meaningful for pipelined (RR would still await anyway).
            if mode == "pipelined":
                c_m, c_p50, c_p99 = await native_cb_pipelined(uri, size, N)
            else:
                c_m = c_p50 = c_p99 = float("nan")
            if PICOWS:
                p_m, p_p50, p_p99 = await fn_p(uri, size, N)
            else:
                p_m = p_p50 = p_p99 = float("nan")
            best = min((n_m, "await"), (c_m, "cb") if mode == "pipelined" else (float("inf"), ""), (p_m, "picows"))
            print(
                f"{size:>6}B {mode:>12} "
                f"{n_m:6.3f}/{n_p50:5.3f}/{n_p99:5.3f}     "
                f"{c_m:6.3f}/{c_p50:5.3f}/{c_p99:5.3f}     "
                f"{p_m:6.3f}/{p_p50:5.3f}/{p_p99:5.3f}   "
                f"← {best[1]}"
            )


async def main():
    # Pin client (this process) to core 1; servers are pinned to core 0 above.
    # Same CCD on Ryzen 9950X for L3 sharing without core-migration noise.
    try:
        os.sched_setaffinity(0, {1})
    except Exception:
        pass
    servers = [
        ("tokio-tungstenite", "rust", "ws_echo_server", 8830),
        ("fastwebsockets", "rust", "ws_echo_fastws", 8831),
        ("picows", "picows", None, 8832),
    ]
    procs = []
    try:
        for label, kind, bin_name, port in servers:
            if kind == "rust":
                procs.append(start_rust_server(bin_name, port))
            else:
                procs.append(start_picows_server(port))
        await asyncio.sleep(0.3)
        for label, _, _, port in servers:
            await run_against(label, port)
    finally:
        for p in procs:
            try:
                if isinstance(p, subprocess.Popen):
                    p.send_signal(signal.SIGTERM)
                    p.wait(timeout=3)
                else:
                    p.terminate()
                    p.join(timeout=3)
            except Exception:
                pass


if __name__ == "__main__":
    asyncio.run(main())
