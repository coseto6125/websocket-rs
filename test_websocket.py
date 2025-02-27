import asyncio
from websocket_rs import WebSocket

async def main():
    # 使用所有新參數
    # connect_timeout: 連線超時（秒）
    # receive_timeout: 接收超時（秒）
    # retry: 是否自動重試連線
    # retry_limit: 重試次數上限
    async with WebSocket(
        "wss://ws.postman-echo.com/raw",
        connect_timeout=5.0,      # 5 秒連線超時
        receive_timeout=10.0,     # 10 秒接收超時
        retry=True,               # 啟用自動重試
        retry_limit=3             # 最多重試 3 次
    ) as ws:
        print(f"已連線，重試次數：{ws.retry_count}")
        
        # 發送訊息
        message = "你好，WebSocket！"
        print(f"發送訊息：{message}")
        await ws.send(message)
        
        # 接收訊息
        response = await ws.receive()
        print(f"收到回應：{response}")
        
        # 如果需要，可以手動重連
        # await ws.reconnect()
        # print(f"已重新連線，重試次數：{ws.retry_count}")

if __name__ == "__main__":
    asyncio.run(main())
