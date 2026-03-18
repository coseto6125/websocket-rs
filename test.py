import asyncio
import uvloop
import json
from websocket_rs.async_client import connect


async def main():
    uri = "wss://stream.binance.com:9443/stream?streams=btcusdt@aggTrade/ethsudt@aggTrade/solusdt@aggTrade/xrpusdt@aggTrade/hypeusdt@aggTrade/bnbusdt@aggTrade/dogeusdt@aggTrade"

    # test_headers = {
    #     "User-Agent": "ArbitrageBot/1.0",
    #     "X-Chainlink-Auth": "SuperSecretToken",
    #     "Origin": "https://localhost",
    # }

    # Using the exact proxy port from your curl test
    test_proxy = "socks5://127.0.0.1:9001"

    print(f"🚀 Initializing Custom Rust WebSocket Engine...")
    print(f"🔗 Target : {uri}")
    print(f"🕵️  Proxy  : {test_proxy}")
    # print(f"📝 Headers: {test_headers}\n")

    try:
        # THE FIX: Just await the connect function directly!
        ws = await connect(uri, proxy=test_proxy)

        # print("✅ CONNECTION ESTABLISHED!")

        # payload = json.dumps({"status": "proxy_test", "message": "Hello from Rust!"})
        # print(f"⬆️  Sending:  {payload}")

        # Send through the proxy
        # await ws.send(payload)

        # Await the echo response
        while True:
            response = await ws.recv()
            print(f"⬇️  Received: {response}")

        # Cleanly close the TLS socket
        await ws.close()

    except Exception as e:
        print(f"\n❌ Connection Failed: {e}")


if __name__ == "__main__":
    uvloop.run(main())
