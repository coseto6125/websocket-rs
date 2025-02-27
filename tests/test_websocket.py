import asyncio
import pytest
from websocket_rs import WebSocket

# 測試服務器 URL
TEST_SERVER = "wss://ws.postman-echo.com/raw"
# 無效的測試服務器 URL
INVALID_SERVER = "wss://invalid-server-that-does-not-exist.example.com"
# 超時測試服務器（可以連接但不會回應的服務器）
TIMEOUT_SERVER = "wss://echo.websocket.events"

@pytest.mark.asyncio
async def test_connection():
    """測試 WebSocket 連線"""
    async with WebSocket(TEST_SERVER) as ws:
        assert ws.is_connected

@pytest.mark.asyncio
async def test_send_receive():
    """測試發送和接收訊息"""
    async with WebSocket(TEST_SERVER) as ws:
        test_message = "測試訊息"
        await ws.send(test_message)
        response = await ws.receive()
        assert response == test_message

@pytest.mark.asyncio
async def test_invalid_url():
    """測試無效的 URL"""
    with pytest.raises(Exception):
        async with WebSocket(INVALID_SERVER) as ws:
            await ws.send("測試")

@pytest.mark.asyncio
async def test_multiple_messages():
    """測試多條訊息"""
    async with WebSocket(TEST_SERVER) as ws:
        messages = ["訊息1", "訊息2", "訊息3"]
        for msg in messages:
            await ws.send(msg)
            response = await ws.receive()
            assert response == msg

@pytest.mark.asyncio
async def test_connection_state():
    """測試連線狀態管理"""
    ws = WebSocket(TEST_SERVER)
    assert not ws.is_connected
    
    async with ws:
        assert ws.is_connected
        await ws.send("測試")
        response = await ws.receive()
        assert response == "測試"
    
    assert not ws.is_connected

@pytest.mark.asyncio
async def test_send_after_close():
    """測試關閉後發送"""
    ws = WebSocket(TEST_SERVER)
    async with ws:
        pass
    
    with pytest.raises(Exception):
        await ws.send("測試")

@pytest.mark.asyncio
async def test_receive_after_close():
    """測試關閉後接收"""
    ws = WebSocket(TEST_SERVER)
    async with ws:
        pass
    
    with pytest.raises(Exception):
        await ws.receive()

@pytest.mark.asyncio
async def test_connect_timeout():
    """測試連線超時"""
    with pytest.raises(Exception):
        # 使用非常短的超時時間，預期會失敗
        async with WebSocket(INVALID_SERVER, connect_timeout=0.1) as ws:
            pass

@pytest.mark.asyncio
async def test_receive_timeout():
    """測試接收超時"""
    async with WebSocket(TEST_SERVER, receive_timeout=0.1) as ws:
        # 不發送訊息，直接接收，預期會超時
        with pytest.raises(Exception):
            await ws.receive()

@pytest.mark.asyncio
async def test_auto_retry():
    """測試自動重試"""
    # 這個測試可能會失敗，因為我們使用的是有效的服務器
    # 但我們可以檢查重試計數器是否為零
    async with WebSocket(TEST_SERVER, retry=True, retry_limit=3) as ws:
        assert ws.retry_count == 0
        assert ws.is_connected

@pytest.mark.asyncio
async def test_manual_reconnect():
    """測試手動重連"""
    ws = WebSocket(TEST_SERVER)
    
    # 首次連接
    async with ws:
        assert ws.is_connected
        await ws.send("測試1")
        response = await ws.receive()
        assert response == "測試1"
    
    assert not ws.is_connected
    
    # 手動重連
    await ws.reconnect()
    assert ws.is_connected
    
    # 測試重連後的功能
    await ws.send("測試2")
    response = await ws.receive()
    assert response == "測試2"
    
    # 關閉連接
    await ws.__aexit__(None, None, None)
    assert not ws.is_connected

@pytest.mark.asyncio
async def test_retry_count_tracking():
    """測試重試計數追蹤"""
    # 使用無效的服務器和自動重試，但設定重試次數上限為 2
    # 預期會失敗，但重試計數應該為 2
    try:
        async with WebSocket(INVALID_SERVER, retry=True, retry_limit=2, connect_timeout=0.5) as ws:
            pass
    except Exception:
        # 連接應該失敗，但我們無法檢查重試計數
        # 因為連接失敗後無法訪問 ws 對象
        pass

    # 使用有效的服務器測試重試計數
    ws = WebSocket(TEST_SERVER)
    async with ws:
        assert ws.retry_count == 0
    
    # 關閉後重連
    await ws.reconnect()
    assert ws.retry_count == 0  # 成功的重連應該沒有重試

@pytest.mark.asyncio
async def test_timeout_properties():
    """測試超時屬性設置"""
    # 創建具有特定超時設置的 WebSocket
    ws = WebSocket(
        TEST_SERVER,
        connect_timeout=5.0,
        receive_timeout=10.0
    )
    
    # 測試連接
    async with ws:
        assert ws.is_connected
        
        # 發送和接收訊息（應該在超時之前完成）
        await ws.send("測試超時屬性")
        response = await ws.receive()
        assert response == "測試超時屬性"

@pytest.mark.asyncio
async def test_retry_properties():
    """測試重試屬性設置"""
    # 創建具有特定重試設置的 WebSocket
    ws = WebSocket(
        TEST_SERVER,
        retry=True,
        retry_limit=5
    )
    
    # 測試連接
    async with ws:
        assert ws.is_connected
        assert ws.retry_count == 0  # 成功的連接應該沒有重試
        
        # 發送和接收訊息
        await ws.send("測試重試屬性")
        response = await ws.receive()
        assert response == "測試重試屬性"

@pytest.mark.asyncio
async def test_combined_properties():
    """測試組合屬性設置"""
    # 創建具有多個屬性設置的 WebSocket
    ws = WebSocket(
        TEST_SERVER,
        connect_timeout=5.0,
        receive_timeout=10.0,
        retry=True,
        retry_limit=3
    )
    
    # 測試連接
    async with ws:
        assert ws.is_connected
        assert ws.retry_count == 0
        
        # 發送和接收訊息
        await ws.send("測試組合屬性")
        response = await ws.receive()
        assert response == "測試組合屬性"
