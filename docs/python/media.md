# Python Resources And Media

Resources and media APIs keep host-owned objects out of serialized runtime
state. Starweaver stores references and metadata; the provider or host remains
responsible for lifecycle and bytes.

## Resource References

`ResourceRef` is a stable provider or host-owned resource reference:

```python
from starweaver import InstructableResource, ResourceRef, ResumableResource


image = ResourceRef.typed(
    "resource://host/image.png",
    kind="image",
    metadata={"media_type": "image/png"},
)
assert image.kind == "image"


class ImageArtifact(ResumableResource, InstructableResource):
    def get_instructions(self):
        return "Use image artifacts only by resource URI."


artifact = ImageArtifact(
    "resource://host/image.png",
    kind="image",
    metadata={"media_type": "image/png"},
)
assert artifact.to_ref().uri == image.uri
assert ImageArtifact.from_state(artifact.export_state()).kind == "image"
```

`ResourceRegistry` is a small in-process registry for host-visible references:

```python
from starweaver import ResourceRegistry, ResourceRegistryState


registry = ResourceRegistry([image])
assert registry.get("resource://host/image.png") is not None

state = registry.state()
assert isinstance(state, ResourceRegistryState)
restored = ResourceRegistry.from_state(state)
```

When a product owns live resource handles, bind them through an explicit
factory. The serialized registry state still contains only `ResourceRef`
records:

```python
from starweaver import EnvironmentProvider, ResourceRegistry


environment = EnvironmentProvider.virtual(files={"README.md": "workspace"})


def build_resources(env):
    return [artifact]


registry = ResourceRegistry.from_factory(build_resources, environment=environment)
assert registry.live("resource://host/image.png") is artifact
assert registry.instructions() == ["Use image artifacts only by resource URI."]

restored = ResourceRegistry.restore(
    registry.state(),
    lambda state, env: [
        ImageArtifact.from_state(state.resources[0].to_dict()),
    ],
    environment=environment,
)
```

`ResourceRegistry.instructions()` only returns instruction strings from live
resources. Product code must explicitly attach those strings to an agent,
capability, or toolset when they should become model-facing.

Use resource references in environment state, media upload responses, and
application-level records. Do not treat them as live provider handles.
`ResourceRegistryState` stores only serializable `ResourceRef` values; products
must restore any live provider or resource handles through their own factory or
environment binding.
`InputPart.file(...)` and `InputPart.binary(...)` accept `ResourceRef` values
and emit canonical durable input JSON, inferring media fields from reference
metadata when present.

## Media Uploaders

`MediaUploader` adapts a Python sync or async callback into the native
`media_upload` model-message filter. The callback receives
`MediaUploadRequest` with bytes, media type, and preflight evidence, and returns
a URL, data URL, or resource-ref content mapping.

```python
from starweaver import MediaUploader, RuntimeConfig, create_agent


async def upload(request):
    return {
        "uri": "resource://host/image.png",
        "media_type": request.media_type,
        "resource_type": "image",
    }


agent = create_agent(
    model=model,
    runtime_config=RuntimeConfig(capabilities=["image_url"]),
    media_uploader=MediaUploader(upload),
)
```

Use `MediaUploader.resource_store(...)` when a product already has a storage
object. The store can be callable or expose `put(request)` and may return a URI
string or a mapping with `id`, `uri`, `url`, or `data_url`:

```python
class Store:
    async def put(self, request):
        return {"id": "image-1", "metadata": {"scope": "workspace"}}


agent = create_agent(
    model=model,
    runtime_config=RuntimeConfig(capabilities=["image_url"]),
    media_uploader=MediaUploader.resource_store(
        Store(),
        uri_prefix="resource://workspace-media",
        resource_type="image",
    ),
)
```

The upload callback is process-local and must be re-registered after session
restore. If upload fails, the run continues with the original or policy-filtered
content and records the failure in request metadata under
`starweaver_media_upload_failures`. The failure message is diagnostic evidence;
the upload adapter should keep private storage URLs and redaction details out of
model-visible content.

## Runtime Media Limits

`RuntimeConfig` carries media-related model-message filter settings:

```python
from starweaver import RuntimeConfig


runtime_config = RuntimeConfig(
    max_images=4,
    max_image_bytes=4_000_000,
    split_large_images=True,
    image_split_max_height=1800,
    image_split_overlap=120,
    capabilities=["image_url"],
)
```

Attach it at agent construction so the same media policy is applied before
every model request.
