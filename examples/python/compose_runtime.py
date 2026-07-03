import asyncio

from pydantic import BaseModel
from starweaver import (
    EnvironmentProvider,
    OutputPolicy,
    OutputSchema,
    Toolset,
    create_agent,
    environment_toolsets,
    tool,
)
from starweaver.testing import TestModel


class Summary(BaseModel):
    status: str
    source: str


@tool
async def summarize_source(path: str) -> dict[str, str]:
    return {"path": path, "summary": "available"}


async def main() -> None:
    environment = EnvironmentProvider.virtual(files={"README.md": "hello from starweaver"})
    workspace = Toolset(
        "workspace",
        tools=[summarize_source],
        instructions=["Use workspace tools for local files."],
    )
    agent = create_agent(
        model=TestModel.text('{"status":"ok","source":"README.md"}'),
        environment=environment,
        toolsets=[workspace, *environment_toolsets()],
        output_policy=OutputPolicy.structured(OutputSchema.from_pydantic(Summary)),
    )
    result = await agent.run("Summarize README.md as JSON.")
    print(result.structured_output)


if __name__ == "__main__":
    asyncio.run(main())
