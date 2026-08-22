
import asyncio as _asyncio
import socket as _socket


def _recv_exact(sock, size, stage):
    chunks = []
    remaining = size
    while remaining:
        chunk = sock.recv(remaining)
        if not chunk:
            raise ConnectionError(f"SOCKS5 proxy closed during {stage}")
        chunks.append(chunk)
        remaining -= len(chunk)
    return b"".join(chunks)


def _socks5_connect_blocking(proxy_host, proxy_port, user, password, target_host, target_port):
    """Blocking SOCKS5 CONNECT. Designed to run inside loop.run_in_executor so it
    never blocks the asyncio event loop. Returns a connected, non-blocking socket
    tunnelled through the proxy to (target_host, target_port)."""
    s = _socket.socket(_socket.AF_INET, _socket.SOCK_STREAM)
    try:
        s.connect((proxy_host, proxy_port))
        s.setsockopt(_socket.IPPROTO_TCP, _socket.TCP_NODELAY, 1)
        methods = b"\x00" if not user else b"\x00\x02"
        s.sendall(b"\x05" + bytes([len(methods)]) + methods)
        reply = _recv_exact(s, 2, "greeting")
        if reply[0] != 0x05:
            raise ConnectionError("SOCKS5 proxy rejected greeting")
        method = reply[1]
        if method == 0x02:
            if not user:
                raise ConnectionError("SOCKS5 proxy requires auth but none supplied")
            ub, pb = user.encode(), password.encode()
            s.sendall(b"\x01" + bytes([len(ub)]) + ub + bytes([len(pb)]) + pb)
            ar = _recv_exact(s, 2, "authentication")
            if ar[1] != 0x00:
                raise ConnectionError("SOCKS5 auth failed")
        elif method != 0x00:
            raise ConnectionError(f"SOCKS5 proxy selected unsupported method {method}")
        host_b = target_host.encode("idna")
        req = b"\x05\x01\x00\x03" + bytes([len(host_b)]) + host_b + int(target_port).to_bytes(2, "big")
        s.sendall(req)
        hdr = _recv_exact(s, 4, "CONNECT reply")
        if hdr[1] != 0x00:
            raise ConnectionError(f"SOCKS5 CONNECT failed: status={hdr[1]}")
        atyp = hdr[3]
        if atyp == 0x01:
            _recv_exact(s, 4, "IPv4 bind address")
        elif atyp == 0x03:
            nlen = _recv_exact(s, 1, "domain bind length")[0]
            _recv_exact(s, nlen, "domain bind address")
        elif atyp == 0x04:
            _recv_exact(s, 16, "IPv6 bind address")
        else:
            raise ConnectionError(f"SOCKS5 returned unsupported ATYP {atyp}")
        _recv_exact(s, 2, "bind port")
        s.setblocking(False)
        return s
    except Exception:
        s.close()
        raise


def _parse_proxy_uri(proxy):
    # socks5://[user:password@]host:port
    from urllib.parse import urlsplit, unquote
    parts = urlsplit(proxy)
    if parts.scheme not in ("socks5", "socks5h"):
        raise ValueError(f"Only socks5:// proxies are supported (got {parts.scheme})")
    user = unquote(parts.username) if parts.username else None
    password = unquote(parts.password) if parts.password else ""
    if not parts.hostname or not parts.port:
        raise ValueError("SOCKS5 proxy URI must include host and port")
    return parts.hostname, parts.port, user, password


async def _connect_helper(loop, protocol_factory, host, port, is_tls, ssl_ctx,
                          proxy, req_bytes, handshake_fut, client, connect_timeout):
    async def _do():
        kwargs = {}
        if is_tls:
            kwargs["ssl"] = ssl_ctx
            kwargs["server_hostname"] = host
        if proxy:
            proxy_host, proxy_port, user, password = _parse_proxy_uri(proxy)
            sock = await loop.run_in_executor(
                None, _socks5_connect_blocking,
                proxy_host, proxy_port, user, password, host, port,
            )
            # Hand the already-connected socket to asyncio. TLS (if any) runs
            # on top of it; asyncio will perform the TLS handshake itself.
            kwargs["sock"] = sock
            transport, _proto = await loop.create_connection(protocol_factory, **kwargs)
        else:
            transport, _proto = await loop.create_connection(
                protocol_factory, host, port, **kwargs
            )
            try:
                s = transport.get_extra_info("socket")
                if s is not None:
                    s.setsockopt(_socket.IPPROTO_TCP, _socket.TCP_NODELAY, 1)
            except Exception:
                pass
        transport.write(bytes(req_bytes))
        await handshake_fut
        return client
    if connect_timeout is not None:
        return await _asyncio.wait_for(_do(), timeout=connect_timeout)
    return await _do()
