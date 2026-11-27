"""Actual MCP handshake using an isolated local process (no Windows app)."""
import asyncio
import os
import sys

from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client

def test_stdio():
    async def check():
        params = StdioServerParameters(command=sys.executable,
            args=["-m", "sensei_mcp.server"], env={**os.environ, "SENSEI_CONFIG":""})
        async with stdio_client(params) as (read, write):
            async with ClientSession(read, write) as client:
                await client.initialize()
                assert [t.name for t in (await client.list_tools()).tools] == ["sensei_status"]
                result = await client.call_tool("sensei_status")
                assert not result.isError
                assert "continuous_images" in result.content[0].text
    asyncio.run(check())
