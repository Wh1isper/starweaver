import asyncio

from starweaver import create_agent, tool
from starweaver.testing import TestModel


@tool
async def add(left: int, right: int) -> dict[str, int]:
    return {"total": left + right}


async def main() -> None:
    model = TestModel.responses(
        [
            TestModel.tool_call_response(
                [{"id": "call_add", "name": "add", "arguments": {"left": 2, "right": 3}}]
            ),
            {"text": "done"},
        ]
    )
    result = await create_agent(model=model, tools=[add]).run("Add two numbers")
    print(result.output)


if __name__ == "__main__":
    asyncio.run(main())
