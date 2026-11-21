"""测试辅助：三个本地 HTTP CONNECT 代理，各自注入不同的人工延迟，模拟快/中/慢节点。

用法：python3 -I fake_proxy.py [启动延迟秒数]
- 18001 → 约 +50ms（快）
- 18002 → 约 +300ms（中）
- 18003 → 约 +900ms（慢）
可选参数让监听延后 N 秒才就绪（用于模拟内核启动时组测试失败、之后节点才可用的场景）。
"""

import asyncio
import sys

# 端口 → 注入延迟（秒）；差距要远大于网络抖动，保证排序稳定
DELAYS = {18001: 0.05, 18002: 1.5, 18003: 2.5}


async def pump(src: asyncio.StreamReader, dst: asyncio.StreamWriter) -> None:
    try:
        while True:
            data = await src.read(65536)
            if not data:
                break
            dst.write(data)
            await dst.drain()
    except Exception:
        pass
    finally:
        try:
            dst.close()
        except Exception:
            pass


async def handle(reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
    port = writer.get_extra_info("sockname")[1]
    try:
        request_line = await reader.readline()
        if not request_line.startswith(b"CONNECT"):
            return
        while True:
            line = await reader.readline()
            if line in (b"\r\n", b"\n", b""):
                break
        target = request_line.split()[1].decode()
        host, _, port_text = target.rpartition(":")
        await asyncio.sleep(DELAYS.get(port, 0.1))
        try:
            remote_reader, remote_writer = await asyncio.open_connection(host, int(port_text))
        except Exception:
            writer.write(b"HTTP/1.1 502 Bad Gateway\r\n\r\n")
            await writer.drain()
            return
        writer.write(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        await writer.drain()
        await asyncio.gather(
            pump(reader, remote_writer),
            pump(remote_reader, writer),
        )
    except Exception:
        pass
    finally:
        try:
            writer.close()
        except Exception:
            pass


async def main() -> None:
    if len(sys.argv) > 1:
        await asyncio.sleep(float(sys.argv[1]))
    servers = [asyncio.start_server(handle, "127.0.0.1", port) for port in DELAYS]
    await asyncio.gather(*servers)
    print("fake proxies ready", flush=True)
    await asyncio.Event().wait()  # 常驻，由测试进程结束/kill 收尾


asyncio.run(main())
