import asyncio
import os

from starweaver import ModelSettings, ProviderModel, create_agent


async def main() -> None:
    model_id = os.environ.get("STARWEAVER_PY_PROVIDER_MODEL", "oauth@codex:gpt-5.5")
    model = ProviderModel.from_model_id(
        model_id,
        model_settings=ModelSettings(timeout_ms=60_000),
    )
    result = await create_agent(model=model).run(
        "Reply with exactly this token and no punctuation: starweaver-python-ok"
    )
    output = result.output.strip()
    if not output:
        raise RuntimeError("provider returned an empty response")
    print(output)


if __name__ == "__main__":
    asyncio.run(main())
