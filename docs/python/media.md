# Python Resources And Media

Resources and media APIs keep host-owned objects out of serialized runtime
state. Starweaver stores references and metadata; the provider or host remains
responsible for lifecycle and bytes.

## Resource References

`ResourceRef` is a stable provider or host-owned resource reference:

```python
from starweaver import ResourceRef


image = ResourceRef.typed(
    "resource://host/image.png",
    kind="image",
    metadata={"media_type": "image/png"},
)
assert image.kind == "image"
```

`ResourceRegistry` is a small in-process registry for host-visible references:

```python
from starweaver import ResourceRegistry


registry = ResourceRegistry([image])
assert registry.get("resource://host/image.png") is not None
```

Use resource references in environment state, media upload responses, and
application-level records. Do not treat them as live provider handles.

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

The upload callback is process-local and must be re-registered after session
restore.

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
