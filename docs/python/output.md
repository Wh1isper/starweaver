# Python Structured Output

Structured output uses the Rust runtime's output loop and Python ergonomic
helpers for schema construction, validators, and final-output functions.

## Output Schemas

Build `OutputSchema` from a Pydantic model:

```python
from pydantic import BaseModel

from starweaver import OutputPolicy, OutputSchema, create_agent
from starweaver.testing import TestModel


class Answer(BaseModel):
    answer: str


async def run_structured() -> None:
    agent = create_agent(
        model=TestModel.text('{"answer":"Paris"}'),
        output_policy=OutputPolicy.structured(OutputSchema.from_pydantic(Answer)),
    )
    result = await agent.run("answer")
    assert result.structured_output == {"answer": "Paris"}
```

Use `OutputSchema(name, schema, description=..., strict=...)` for hand-written
JSON Schema.

## Output Modes

`OutputPolicy` selects the model-facing output mode:

- `OutputPolicy.text()` for plain text.
- `OutputPolicy.structured(schema)` for structured output.
- `OutputPolicy.auto(schema)` for provider-chosen structured output.
- `OutputPolicy.native_json_schema(schema)` for native JSON schema mode.
- `OutputPolicy.native_json_object(schema)` for native JSON object mode.
- `OutputPolicy.tool(schema)` for tool-based output.
- `OutputPolicy.tool_or_text(schema)` when text fallback is allowed.
- `OutputPolicy.prompted(schema)` for prompt-only structured output.
- `OutputPolicy.image()` for image output flows.

## Validators

Use validators when the model output parses but fails application rules. Raise
`OutputRetry` to request another model turn within the policy retry budget.

```python
from pydantic import BaseModel

from starweaver import OutputPolicy, OutputRetry, OutputSchema, create_agent
from starweaver.testing import TestModel


class Answer(BaseModel):
    answer: str


def require_paris(output: dict[str, object]) -> None:
    if output["answer"] != "Paris":
        raise OutputRetry("answer must be Paris")


async def validate_answer() -> None:
    policy = (
        OutputPolicy.structured(OutputSchema.from_pydantic(Answer))
        .with_validator(require_paris)
        .with_retries(1)
    )
    agent = create_agent(
        model=TestModel.responses(
            [{"text": '{"answer":"London"}'}, {"text": '{"answer":"Paris"}'}]
        ),
        output_policy=policy,
    )
    result = await agent.run("answer")
    assert result.structured_output == {"answer": "Paris"}
```

Validators can accept an `OutputContext` first parameter when they need runtime
metadata.

## Output Functions

`OutputFunction` exposes a final-output function to the model. The callback can
return a string for text output, a JSON-serializable value for structured
output, or `OutputValue.text()` / `OutputValue.json()` when the boundary should
be explicit.

```python
from starweaver import OutputFunction, OutputPolicy, create_agent
from starweaver.testing import TestModel


def final_answer(args: dict[str, object]) -> dict[str, object]:
    return {"answer": args["answer"]}


async def run_output_function() -> None:
    final = OutputFunction(
        "final_answer",
        {
            "type": "object",
            "properties": {"answer": {"type": "string"}},
            "required": ["answer"],
        },
        final_answer,
    )
    agent = create_agent(
        model=TestModel.responses(
            [
                TestModel.tool_call_response(
                    [
                        {
                            "id": "call_final",
                            "name": "final_answer",
                            "arguments": {"answer": "Paris"},
                        }
                    ]
                )
            ]
        ),
        output_policy=OutputPolicy().with_function(final),
    )
    result = await agent.run("answer")
    assert result.structured_output == {"answer": "Paris"}
```

## Per-Run Output

Output policy can be attached at agent construction or per run:

```python
result = await agent.run(
    "return JSON",
    output_policy=OutputPolicy.structured(OutputSchema.from_pydantic(Answer)),
)
```

Use per-run output when one agent serves multiple endpoint shapes.
