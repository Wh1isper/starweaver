# IDL-First JSON-RPC Host Protocol and Client Generation

Status: accepted target architecture; implementation planned

Revision: 2026-07-21

This specification defines the next canonical Starweaver host protocol as an IDL-first JSON-RPC design. It deliberately does not preserve the structural limitations of the implemented Rust-first v1 wire contract. The current v1 host remains documented in `06-json-rpc-host-protocol.md` until the new major is implemented and cut over.

## Decision

Starweaver adopts the following protocol stack for the next host-protocol major:

- JSON-RPC 2.0 is the wire protocol.
- OpenRPC 1.4.0 plus its JSON Schema Draft 7 dialect is the canonical language-neutral IDL.
- The new contract uses protocol identity `starweaver.host` major `2`.
- Checked-in IDL source generates the public bundled OpenRPC document, Rust server bindings, a complete TypeScript protocol model, and manifest-filtered safe Desktop bridge/client bindings.
- `starweaver-rpc` implements the generated Rust server boundary.
- Rust and TypeScript outputs are generated peers; neither is an independent protocol source.
- The current handwritten v1 DTOs, method tables, and corpus are behavioral inventory for the redesign, not wire-compatibility requirements for major 2.
- Protobuf, Thrift, gRPC, TypeSpec, and language-first schema derivation are not part of this protocol.

The IDL is authoritative for structural wire facts: methods, notifications, params, results, public errors, field names, enum values, requiredness, nullability, bounds, type openness, feature metadata, transport availability, and idempotency classification. Behavioral specifications remain authoritative for durability, ordering, authorization, fencing, process supervision, replay recovery, and shutdown barriers.

## Rationale

OpenRPC is selected because the product protocol is JSON-RPC. Its method, params, result, error, and example model maps directly to the actual wire without a Protobuf-JSON, Thrift-JSON, or language-specific translation layer.

This choice preserves the product properties that matter for Starweaver:

- one readable protocol over local child stdio and SSH-carried stdio;
- simple implementation by independent clients in any language;
- direct golden-frame and invalid-frame conformance testing;
- natural representation of tagged agent events and bounded extension data;
- deterministic Rust and TypeScript generation from one public artifact; and
- future HTTP or WebSocket transport profiles without changing method schemas.

OpenRPC is an IDL, not an execution runtime. Starweaver owns the constrained schema profile, generators, transport profiles, subscription behavior, feature negotiation, authorization, durability, and compatibility gates.

## Goals

- Define a clean host contract from the IDL outward rather than transcribing Rust types.
- Generate all canonical Rust wire types and server signatures.
- Generate a complete external TypeScript protocol model and runtime codecs.
- Generate a separate least-authority Desktop bridge/client surface from the IDL plus a reviewed authority manifest.
- Make the bundled OpenRPC artifact sufficient for an independent Go, Python, Swift, Kotlin, or other client.
- Eliminate JSON precision ambiguity, implicit defaults, trial-deserialized unions, untyped errors, and accidental open objects.
- Use one durable event vocabulary for replay and live delivery.
- Make same-major compatibility mechanically enforceable.
- Keep JSON-RPC transport profiles independent from domain handlers and storage implementations.

## Non-goals

- Preserving every valid v1 frame, alias, numeric range, or DTO shape.
- Generating runtime, storage, OAuth, environment, authorization, or handler behavior.
- Exposing internal Rust domain records directly because they are serializable.
- Treating OpenRPC as a complete durability, security, or process-lifecycle specification.
- Giving the Desktop renderer a raw host transport, complete host params, credentials, paths, SSH authority, or generic JSON-RPC dispatch.
- Using Protobuf or Thrift merely as an alternate schema language over JSON-RPC.
- Introducing TypeSpec as an authoring frontend before direct OpenRPC authoring demonstrates a concrete maintenance problem.
- Shipping first-party SDKs for every language in the initial implementation.

## Relationship to Host Protocol v1

The implemented v1 contract remains valid only for v1 clients and servers while it is supported. Major 2 is intentionally a new wire contract.

The redesign follows these rules:

- v1 behavior is inventoried method by method so required product capabilities are not lost;
- v1 field names and structures may be retained when they are already clear and cross-language safe;
- legacy method aliases, field aliases, omitted-params normalization, integer request IDs, and untyped errors do not carry into major 2;
- v1 fixtures remain under the v1 contract and are never rewritten to pretend they were major 2;
- major 2 receives an independent canonical and invalid corpus generated from its IDL;
- a temporary server may expose both majors through explicit protocol dispatch during cutover, but no frame is interpreted as both; and
- Desktop execution remains disabled until it can require and validate the new major and its mandatory features.

If product release policy establishes that v1 was never public, implementation may replace it atomically rather than maintain dual-major runtime support. That release decision does not change the major-2 contract defined here.

## Ownership

| Concern                                                                                       | Owner                                                               |
| --------------------------------------------------------------------------------------------- | ------------------------------------------------------------------- |
| Canonical host IDL source and public bundled artifact                                         | repository-level `protocol/starweaver-host/v2/` tree                |
| IDL profile, bundling, linting, compatibility checks, and generators                          | repository `xtask` automation                                       |
| Generated Rust wire types, server trait, dispatcher, and protocol codecs                      | `starweaver-rpc-core`                                               |
| Rust handlers, authorization, coordination, subscriptions, and transports                     | `starweaver-rpc`                                                    |
| Complete generated TypeScript protocol model and codecs                                       | repository generator output, excluded from Desktop renderer imports |
| Desktop operation authority manifest and generated safe TypeScript bridge/client              | Starweaver Desktop                                                  |
| Child process, stdio/SSH transport, request identity, retries, replay recovery, and authority | Desktop privileged Rust backend                                     |
| Renderer intents, safe view models, and presentation state                                    | Desktop TypeScript application layer                                |
| Durable and product-neutral domain semantics                                                  | existing owning crates and specs                                    |
| Implemented major-1 behavior                                                                  | `06-json-rpc-host-protocol.md`                                      |
| Desktop process and connection lifecycle                                                      | `../desktop/` specifications                                        |

Generated wire types never import Rust crate paths. Handwritten adapters convert between generated wire projections and product-neutral domain types. This conversion boundary prevents storage or runtime implementation details from becoming accidental public protocol.

## Canonical Artifact Layout

```text
protocol/
  starweaver-host/
    v2/
      openrpc.yaml
      schemas/
        common.yaml
        initialize.yaml
        sessions.yaml
        runs.yaml
        events.yaml
        interactions.yaml
        environments.yaml
        errors.yaml
      examples/
        initialize.json
        run-start.json
        events-subscribe.json
        host-event.json
        typed-error.json
      generated/
        starweaver-host-v2.openrpc.json
apps/
  starweaver-desktop/
    protocol/
      host-surface.yaml
```

Rules:

- Split YAML files are the human-maintained canonical source.
- The self-contained JSON bundle is the public release artifact and contains no local file references.
- `apps/starweaver-desktop/protocol/host-surface.yaml` is Desktop-owned authority and projection policy over public IDL symbols, not a second host schema or part of the public host bundle.
- Generated Rust and TypeScript files are committed for review and reproducibility but never edited manually.
- Examples are conformance evidence, not type inference input.
- The bundle records protocol family, major, revision, and a SHA-256 artifact digest.
- The digest is computed over deterministic bundled bytes with the digest member omitted.
- Generated code records the same identity and digest.

No new workspace crate is required merely to hold source files. A publishable package is introduced only when there is a concrete distribution boundary.

## OpenRPC Profile

The canonical source and public bundle use OpenRPC 1.4.0 and its JSON Schema Draft 7 Schema Object dialect. CI validates the bundle against the pinned official OpenRPC meta-schema and loads it with at least one independent OpenRPC parser.

Starweaver supports a constrained subset:

- named component schemas and local `$ref` references;
- closed and explicitly open objects;
- required and optional properties;
- strings with enum, pattern, length, and documented format constraints;
- booleans;
- bounded JSON-safe integers only where number semantics are intentional;
- arrays with one item schema and explicit bounds where required;
- maps with an explicit value schema;
- tagged `oneOf` unions with one stable discriminator;
- explicit null unions;
- descriptions, examples, and deprecation metadata; and
- reviewed `x-starweaver-*` extensions.

Unsupported or ambiguous constructs fail generation. Generators must never silently widen a type to Rust `serde_json::Value`, TypeScript `any`, or an unvalidated object.

### Direct Authoring

OpenRPC and JSON Schema are authored directly. TypeSpec is not an additional source layer in the initial architecture. This keeps the public contract identical to the canonical structural source and avoids maintaining a custom TypeSpec-to-OpenRPC semantic lowering.

A future authoring frontend is acceptable only if:

- the generated OpenRPC bundle remains the public compatibility artifact;
- lowering is deterministic and semantics-preserving;
- source and bundle changes are reviewed together; and
- every generator and fixture continues to consume the bundled contract.

### Starweaver Extensions

Only documented `x-starweaver-*` extensions are accepted. The initial profile defines extensions for:

- generated aggregate params type names;
- server-to-client notification declarations;
- required feature, transport, authorization scope, idempotency class, and behavioral-spec links;
- protocol identity and schema digest;
- canonical decimal kind and string-valued minimum/maximum;
- a root-level `x-starweaver-error-data` registry mapping every error code and `data.kind` to one schema; and
- explicit extension-map trust and size metadata where JSON Schema alone is insufficient.

Unknown Starweaver extensions fail linting. Language-specific code snippets, Rust paths, TypeScript import paths, and handler implementation metadata are prohibited in the public IDL.

## JSON-RPC Major-2 Profile

### Envelope

Major 2 narrows JSON-RPC 2.0 to one deterministic profile:

- `jsonrpc` is exactly `"2.0"`;
- client request IDs are non-empty strings;
- numeric, `null`, boolean, array, and object client request IDs are rejected;
- successful and method-level error responses echo the exact string request ID;
- only parse/invalid-request errors for which no valid request ID can be recovered use response `id: null`, as required by JSON-RPC 2.0;
- every client invocation includes a valid string `id`; client-to-server notifications are unsupported and never dispatch a method;
- every request includes object-valued `params`, including `{}` for empty params;
- positional params, scalar params, `null` params, and omitted params are rejected;
- batch arrays are rejected as one invalid-request error with response `id: null`, not processed element by element;
- requests and responses contain no unknown envelope members;
- success responses contain `result` and never `error`;
- error responses contain `error` and never `result`; and
- server notifications omit `id` and contain exactly `jsonrpc`, `method`, and object-valued `params`.

Canonical request:

```json
{
  "jsonrpc": "2.0",
  "id": "req_01J2Y8M4YR2E6P3N5V7T9K1ABC",
  "method": "run.start",
  "params": {
    "sessionId": "sess_01J2Y8M0T4FQW9W2R7CBM6A3K8",
    "input": {
      "parts": [
        {
          "kind": "text",
          "text": "Summarize the repository"
        }
      ]
    },
    "idempotencyKey": "idem_01J2Y8ME0R4J9M8C7Q1B5W6P3A"
  }
}
```

String request IDs eliminate JavaScript precision ambiguity and remain distinct from durable operation identity.

### Params and Results

Every method sets `paramStructure: by-name`. The standard OpenRPC `params` array contains one Content Descriptor per top-level params property. `x-starweaver-params-type` names the generated aggregate object without adding an extra wire nesting level.

```yaml
methods:
  - name: session.get
    paramStructure: by-name
    x-starweaver-params-type: SessionGetParams
    params:
      - name: sessionId
        required: true
        schema:
          $ref: "#/components/schemas/SessionId"
      - name: includeRuns
        required: false
        schema:
          type: boolean
    result:
      name: session
      schema:
        $ref: "#/components/schemas/SessionGetResult"
```

This generates params `{ sessionId, includeRuns? }` and the frame member `"params":{"sessionId":"..."}`. It never generates `"params":{"params":{...}}`.

Objects are closed by default with `additionalProperties: false`. An open map is allowed only through a named component whose value schema and trust semantics are explicit.

Missing, `null`, empty, and defaulted values are distinct. The IDL must never rely on a language default that is not visible in the wire contract. Canonical writers omit an optional member only when omission is explicitly part of its schema.

### Identifiers

Business identifiers are opaque, non-empty strings with named schemas:

- `SessionId`;
- `RunId`;
- `SubscriptionId`;
- `EventId`;
- `DiagnosticId`;
- `IdempotencyKey`; and
- environment and resource identifiers.

Clients compare identifiers for equality only. They do not infer timestamps, routing, tenancy, or authorization from a textual prefix.

### Large Integers

All potentially 64-bit counters, revisions, generations, offsets, and byte sizes use canonical unsigned decimal strings on the JSON wire. Durable host-event cursors are opaque scope/view-bound tokens rather than exposed numeric positions; per-subscription delivery sequence numbers use `DecimalU64`.

```yaml
DecimalU64:
  type: string
  pattern: "^(0|[1-9][0-9]{0,19})$"
  x-starweaver-decimal-kind: uint64
  x-starweaver-decimal-minimum: "0"
  x-starweaver-decimal-maximum: "18446744073709551615"
```

The reviewed decimal extensions are part of the public Starweaver OpenRPC profile and are enforced by every generator and runtime validator; they compensate for Draft 7 having no numeric comparison for decimal strings. Generated Rust APIs expose an appropriate integer newtype. Generated TypeScript APIs expose `bigint` or a branded decimal type through generated codecs. Plain JSON `number` is reserved for values whose complete declared domain is within JavaScript's safe integer range.

### Timestamps and Durations

Timestamps use normalized RFC 3339 UTC strings. Durations use named decimal-millisecond or ISO 8601 string schemas selected per field; one field never accepts both. Generated code performs range and canonical-form validation.

### Tagged Unions

Every control-flow union uses a required string discriminator named `kind`. Variant selection never depends on trial deserialization order or field coincidence.

```json
{
  "kind": "approval_requested",
  "approval": {
    "approvalId": "approval_01J2Y8..."
  }
}
```

A new variant is same-major compatible only when it is tied to a newly negotiated feature and the server never sends it to clients that did not advertise that feature. Otherwise adding a variant requires a new major.

### JSON Values and Extension Maps

Unconstrained JSON uses one explicit recursive `JsonValue` component. Each field using it documents:

- producer and consumer trust;
- redaction requirements;
- whether it may be persisted;
- whether unknown values may be forwarded; and
- size and depth bounds.

New public fields should prefer typed schemas. Provider responses, credentials, HTTP headers, shell commands, and unrestricted paths must not be hidden inside generic JSON.

## Method Model

Every method declares:

- exact method name;
- aggregate params type;
- result type;
- possible public errors;
- required feature, if any;
- allowed transport profiles;
- authorization scope;
- idempotency class;
- stability state; and
- behavioral-spec references.

Major 2 has one canonical name per method and field. It has no legacy read aliases. A deprecated method remains implemented for the entire major and can be removed only in a later major.

Method groups remain product-oriented rather than transport-oriented:

- initialization and capabilities;
- sessions;
- runs and continuation;
- events and replay;
- approvals, deferred work, and clarification;
- model/profile selection;
- environment attachments;
- search and bounded discovery;
- OAuth and runtime management when publicly authorized; and
- coordinated shutdown.

The exact catalog is authored in the IDL and generated into both server and client method maps. Handwritten registries are prohibited after cutover. Host methods are bounded control operations: starting a run returns promptly, long-running work progresses through durable events, and major 2 does not expose an unbounded `run.await` request. Explicit run interruption is a domain method and is distinct from abandoning local interest in an RPC response.

## Initialization and Feature Negotiation

Major 2 defines initialization cleanly instead of extending the closed v1 shape.

Request:

```json
{
  "jsonrpc": "2.0",
  "id": "req_01J2Y8...",
  "method": "initialize",
  "params": {
    "client": {
      "name": "starweaver-desktop",
      "version": "1.0.0"
    },
    "protocol": {
      "name": "starweaver.host",
      "major": 2
    },
    "supportedFeatures": [
      "events.replay.v1",
      "events.subscribe.v1",
      "interaction.approval.v1"
    ],
    "requiredFeatures": [
      "events.replay.v1"
    ]
  }
}
```

Result:

```json
{
  "jsonrpc": "2.0",
  "id": "req_01J2Y8...",
  "result": {
    "server": {
      "name": "starweaver-rpc",
      "version": "1.0.0"
    },
    "protocol": {
      "name": "starweaver.host",
      "major": 2,
      "revision": "2026-07-21",
      "schemaDigest": "sha256:..."
    },
    "supportedFeatures": [
      "events.replay.v1",
      "events.subscribe.v1",
      "interaction.approval.v1"
    ],
    "negotiatedFeatures": [
      "events.replay.v1",
      "events.subscribe.v1",
      "interaction.approval.v1"
    ],
    "capabilities": {},
    "runtime": {},
    "storage": {}
  }
}
```

Rules:

- stateful stdio/SSH connections accept only `initialize` before successful initialization;
- protocol family and major must match exactly;
- supported and required feature arrays are duplicate-free and canonically sorted;
- every required feature must also appear in the client's supported set;
- the server rejects initialization when any required feature is unsupported;
- negotiated features are the duplicate-free, canonically sorted intersection of client-supported and server-supported features;
- stateful peers store the negotiated set for the connection;
- feature-gated methods and events on a stateful connection require membership in that set;
- capabilities report effective instance state and never replace vocabulary features;
- revision is an exact artifact identity and is not ordered like a minor version; and
- reconnect identity, runtime identity, storage compatibility, and safe workspace identity are typed members from the first major-2 release rather than later optional additions to a closed result.

Feature IDs are lowercase ASCII tokens matching `^[a-z][a-z0-9._-]*$` and are sorted by ascending unsigned UTF-8 byte sequence, which is equivalent to ASCII lexical order for this alphabet. They are versioned semantic capabilities, not build flags. Enabling server configuration never claims client support. The canonical initialize request/result pair is shared unchanged by Rust, TypeScript, and independent-client fixtures.

## Errors

Every error response contains stable machine-readable public data.

```json
{
  "jsonrpc": "2.0",
  "id": "req_01J2Y8...",
  "error": {
    "code": -32013,
    "message": "run is owned by another host",
    "data": {
      "kind": "foreign_active_owner",
      "retryable": false,
      "reconciliationRequired": true,
      "diagnosticRef": "diag_01J2Y8..."
    }
  }
}
```

Rules:

- standard JSON-RPC parse, invalid-request, method-not-found, invalid-params, and internal-error codes retain their standard meanings;
- Starweaver domain codes and `data.kind` values are stable within a major;
- client control flow uses code and typed data, never message parsing;
- `message` is safe for user display but contains no secrets or internal chain text;
- public data never includes raw provider responses, SQL, headers, credentials, unrestricted paths, or debug backtraces;
- retryability and reconciliation requirements are explicit for effectful operations; and
- unknown codes remain generic failures and never trigger blind mutation retry.

OpenRPC declares common protocol errors and method-specific domain errors. Because the standard OpenRPC Error Object does not attach a schema to `data`, the bundle also carries one root-level registry at an extension point accepted by the official meta-schema:

```yaml
x-starweaver-error-data:
  - name: ForeignActiveOwner
    code: -32013
    kind: foreign_active_owner
    schema:
      $ref: "#/components/schemas/ForeignActiveOwnerErrorData"
```

Registry rules:

- every declared Error Object, including standard JSON-RPC errors, has exactly one registry entry and one typed data schema;
- `name`, `code`, `kind`, and schema reference are unique and internally consistent;
- every data schema is a closed tagged object whose discriminator equals the registered `kind`;
- method `errors` arrays reference only registered Error Objects;
- the Starweaver extension meta-schema, bundle linter, generators, compatibility checker, and fixtures validate the registry; and
- no typed error mapping lives in handwritten Rust or TypeScript tables.

Rust and TypeScript error unions are generated from these components and the registry.

## Idempotency and Mutation Receipts

JSON-RPC request ID is connection-local correlation only. Every effectful method requires a durable `idempotencyKey` in params unless its behavioral spec proves the operation intrinsically idempotent.

The trusted client creates the key before the first send and retains it across retries. The server scopes and fingerprints it, then returns a secret-free mutation receipt containing operation kind, state, target/result reference, and reconciliation status.

Desktop renderer input never supplies or overrides idempotency keys. The privileged Rust supervisor constructs them when mapping safe bridge requests to complete host params.

After response loss, clients query the receipt or target state before retrying. They never retry an effectful operation with a new key merely because the transport disconnected.

## Events, Replay, and Live Delivery

Major 2 separates view-independent durable evidence from its view-bound delivery position. `EventRecord` is the canonical durable event. `EventDelivery` pairs that record with the `HostEventCursor` generated for one admitted logical view; the same delivery schema is used by replay and live notifications.

```json
{
  "cursor": {
    "family": "host_event",
    "scope": {
      "kind": "run",
      "runId": "run_01J2Y8..."
    },
    "token": "cursor_01J2Y8..."
  },
  "record": {
    "eventId": "event_01J2Y8...",
    "occurredAt": "2026-07-21T11:00:00Z",
    "sessionId": "sess_01J2Y8...",
    "runId": "run_01J2Y8...",
    "event": {
      "kind": "run_status_changed",
      "status": "completed"
    }
  }
}
```

Every `EventRecord` is committed to the durable event log before any delivery that references it is emitted. The durable record contains no caller, authority, feature-set, view, or cursor material. Replay and subscription projection create `EventDelivery` values by pairing eligible records with cursors for the admitted `EventView`. The same durable record can therefore appear in multiple logical views with the same `eventId` and payload but a different cursor. Ephemeral transport diagnostics remain on stderr and never masquerade as host events. UI-only presentation models are projections and are not separate durable truth.

`HostEventCursor` is a major-2 public projection independent from the existing internal `ReplayCursor` vocabulary. It has stable family `host_event`, a typed session/run/global scope, and an opaque token. The integrity-checked token is generated while reading/projecting a logical view and binds the durable log position, storage domain, exact scope, event-view digest, and backend cursor family without exposing record counts or backend layout. It survives process restart for the same retained storage evidence and becomes invalid only through typed retention/view/storage mismatch rules. Clients compare cursor identity only; they never increment, order, coerce, or transplant tokens.

The token is a bounded ASCII value, not a credential or authorization grant; every use rechecks current authority and feature eligibility. The start cursor, accepted cursor, replay fence, page `nextCursor`, live record cursors, and terminal cursor for one operation must share family, scope, and event-view digest. A mismatch returns one typed `cursor_scope_mismatch` or `cursor_view_mismatch` error without revealing whether another scope exists. Handwritten adapters may translate between `HostEventCursor` and internal stream/storage cursors, but the IDL never exposes internal cursor structs.

### Atomic Event Publication

Every authoritative transition whose contract requires an event commits the state change and one view-independent `EventRecord` or transactional-outbox entry in one storage transaction. Direct event-log append is preferred when state and log share an atomic store. Otherwise the outbox is durable evidence that recovery drains idempotently into the event log before live emission. Neither path persists an `EventDelivery` or a view-bound `HostEventCursor` as the event evidence.

Each logical transition has a deterministic `eventId`/publication key. Recovery may retry materialization but must produce exactly one semantic replay record. A state transition is never considered fully published merely because mutable state changed; reconciliation detects and completes pending outbox work. Live delivery begins only after the durable `EventRecord` exists; view projection then materializes its `EventDelivery`.

Crash fixtures cover failure before the state transaction, after state/outbox commit but before event-log materialization, and after event-log append but before live emission. The release gate proves that every event-required durable transition eventually has exactly one replayable record.

### Event View and Eligibility

`events.replay` and `events.subscribe` take one typed `EventView` containing the exact scope, one versioned view profile, and requested optional event features. A profile defines the essential event variants required for one coherent projection and the optional enrichments it may include. On a stateful connection, every requested feature must be a subset of negotiated features. Unary HTTP replay computes a request-local intersection from its declared event features and server support.

Every `HostEvent` variant declares in the IDL:

- required feature, if any;
- required authorization scopes;
- whether it is essential in each view profile or an optional enrichment; and
- redaction/projection policy.

The server admits a view only when the caller is authorized for every event class selected by that profile and its optional features. It never admits a broad view and then silently removes unauthorized event classes. Events outside the selected profile are a different logical projection stream: they do not consume its cursor positions, create empty pages, affect `hasMore`, or reveal hidden count/activity.

Replay catch-up and live delivery use the same admitted-view eligibility function. Authorization or negotiated-feature changes invalidate the view and close an existing subscription with a typed reason. A cursor token is bound to the view digest so it cannot be reused under another profile, feature set, or authority projection. Ineligible profile admission fails before returning records with typed `required_feature` or `forbidden_event_scope` data and does not reveal whether a protected target or event exists.

Live `host.event` params include a contiguous per-subscription `deliverySequence: DecimalU64` and one view-bound `EventDelivery`. Every subscription generation starts at `"1"`; `events.subscribe` returns `nextDeliverySequence: "1"`, and the first `host.event` must equal that value. The sequence is allocated only when an eligible event frame is committed to the connection's ordered writer, immediately before that frame; filtered records, internal attempts, and terminal control frames do not consume values. Each subsequent event increments by one without wraparound. Before another value would exceed `DecimalU64`, the server terminates the subscription with a typed `sequence_exhausted` reason rather than emitting an ambiguous event. The client treats a missing, repeated, or unexpected first or later value as a transport gap. Durable recovery always resumes from the client's last applied opaque delivery cursor, not from this connection-local sequence.

### Methods

The initial event control surface is:

- `events.replay` — bounded durable records after an opaque scope/view-bound cursor;
- `events.subscribe` — establish a connection-owned live tail from a declared replay boundary; and
- `events.unsubscribe` — explicitly close a subscription.

`events.replay` returns `{ deliveries, nextCursor, hasMore }`, where each item is an `EventDelivery`. `nextCursor` is always present and advances only within the admitted logical view. `hasMore` describes additional records in that same view; events outside the profile do not affect either field.

Subscription admission atomically registers the eligible live tail and captures a durable replay fence for one `EventView`. `events.subscribe` returns a `subscriptionId`, accepted start cursor, captured fence with the same family/scope/view binding, and `nextDeliverySequence: "1"`. After the successful response crosses the transport flush barrier, the subscription emits eligible retained records in `(start, fence]`, drains eligible records buffered after the fence, and then follows the live durable tail. This transition has no gaps; a correct connection emits each eligible record once, while clients still tolerate duplicates during reconnect and recovery. Per-subscription delivery order follows the contiguous delivery sequence and durable record order.

Backpressure never silently drops records. Response frames and subscription terminal control frames have reserved capacity/priority separate from ordinary event capacity. On overflow, the server stops admission for that subscription and attempts `subscription.closed` with typed overflow reason, `lastFlushedCursor`, and `lastFlushedDeliverySequence` when known. These fields identify the last eligible `host.event` successfully flushed to the transport; they are diagnostic, while recovery uses the client's last applied cursor. Sequence exhaustion follows the same terminal path. If the terminal frame cannot flush within its deadline, the server closes the entire transport so the client recovers from its own last applied cursor. A successful `events.unsubscribe` response is a terminal flush barrier: after it flushes, no `host.event` for that subscription generation may be written.

### Notifications

Server notifications are declared through the reviewed `x-starweaver-notifications` extension:

- `host.event` with `{ subscriptionId, deliverySequence, delivery }`; and
- `subscription.closed` with typed reason plus `lastFlushedCursor` and `lastFlushedDeliverySequence` when known.

Event-specific notification method names such as `run.status` and `diagnostic` are not part of major 2. Their data becomes variants of the typed `HostEvent` union carried by `host.event.delivery.record.event`.

A client applies each delivery idempotently by `delivery.record.eventId` and its last applied `delivery.cursor`. A delivery-sequence gap, cursor binding mismatch, malformed event, or decoder failure closes the tail and triggers bounded replay or projection recovery. Receipt of a valid `host.event` implies the referenced record was already durably committed; it does not imply that the client has acknowledged or rendered it.

stdio supports live subscriptions. Unary HTTP is replay/status-only and rejects subscribe/unsubscribe as unavailable for that transport. A future WebSocket profile may carry the same methods and notifications without changing their schemas.

## Transport Profiles

### Stdio

- UTF-8 NDJSON carries exactly one JSON-RPC frame per non-empty line.
- the first valid request is major-specific `initialize`; its `protocol.major` selects one dispatch table for the connection, and no later frame may change it;
- stdin carries client frames; stdout carries responses and server notifications; stderr carries diagnostics only.
- every output frame is flushed according to the response/notification ordering contract;
- bounded queues and frame-size limits fail closed rather than consume unbounded memory;
- response flush precedes notifications activated by that response;
- EOF initiates coordinated disconnect handling; and
- no logging or bootstrap prose appears on stdout after strict framing begins.

### HTTP

- Major 2 uses the versioned endpoint `POST /rpc/v2`; a dual-major server keeps major 1 on its separately documented endpoint and never infers a major from overlapping method names.
- HTTP carries unary JSON-RPC requests and responses.
- `initialize` is stateless capability discovery: the response returns the request-specific feature intersection, which the client retains locally, but the server does not treat the TCP connection as protocol session state.
- HTTP methods are admitted by endpoint major, server feature availability, authorization, and method schema. Methods or event variants that require connection-held negotiation are unavailable.
- batch remains unsupported.
- live subscription methods are unavailable.
- authentication, origin/host checks, scopes, limits, and replay-only behavior remain transport policy rather than method-schema differences.

### SSH

SSH carries the strict stdio profile after the bounded nonce-marked bootstrap. Shell banners and login output must be consumed or rejected before JSON-RPC framing begins. No remote listener is required.

## Generated Rust Boundary

The Rust generator emits into `starweaver-rpc-core`:

- envelope and string request-ID types;
- method params and result structs;
- opaque identifiers and decimal-integer newtypes;
- enums and discriminated unions;
- typed errors and public error data;
- event records and notification unions;
- protocol identity, feature, transport, authorization, and idempotency metadata;
- exhaustive method and notification identifiers;
- structural validators and canonical codecs;
- an async server trait binding every method to params, result, and declared errors; and
- an exhaustive dispatcher used by `starweaver-rpc`.

Generated code is wire-only. It does not open storage, authorize users, construct agents, allocate environments, own Tokio tasks, or write transport frames. `starweaver-rpc` implements behavior and maps generated DTOs to domain/service types.

Handwritten canonical DTO or method registries are prohibited after cutover. Narrow handwritten transport helpers, domain conversion adapters, and runtime policies remain allowed in their owning modules.

## Generated TypeScript Boundaries

Generation produces two distinct TypeScript surfaces.

### Complete Protocol Model

The complete model contains:

- all wire types and discriminated unions;
- method params/result maps;
- notification maps;
- typed errors;
- decimal-integer and timestamp codecs;
- runtime validators for every received value;
- canonical request encoders;
- a transport-neutral `HostRpcClient`; and
- protocol identity and schema-digest constants.

This model is appropriate for conformance tests and trusted external TypeScript clients. It is not renderer-authorized and must not be imported into the Desktop renderer bundle.

### Safe Desktop Client

The Desktop-owned `apps/starweaver-desktop/protocol/host-surface.yaml` selects operations and classifies fields as:

- renderer-provided;
- supervisor-constructed;
- supervisor-overridden;
- forbidden across the bridge; or
- safe output projection.

Generation combines the IDL and manifest to emit:

- TypeScript `DesktopHostClient` operations;
- safe bridge request/result/notification DTOs and runtime decoders;
- a matching Rust operation enum;
- Rust bridge decoders;
- Rust wire constructors that fill supervisor-owned fields; and
- Rust result/notification redaction and projection code.

The Rust backend never deserializes renderer input as complete host params. It constructs request IDs, idempotency keys, routing identity, client scope, execution-domain binding, and retry metadata. Raw host paths, credentials, private diagnostics, and unselected fields fail closed before crossing into TypeScript state.

Architecture checks reject imports of the complete host model, raw host codecs, `HostRpcClient`, or unfiltered notification unions from renderer code.

## Compatibility and Versioning

Compatibility uses:

- protocol family `starweaver.host`;
- integer major `2`;
- non-ordered exact revision;
- artifact digest; and
- negotiated feature IDs.

Same-major changes may:

- add documentation and examples;
- add a feature-gated method;
- add an optional-enrichment event variant with declared feature/scope metadata that is delivered only through an `EventView` eligible for its newly negotiated feature;
- add a feature-gated error detail that old clients cannot receive without opting in; or
- extend a component explicitly designed as an extension map.

A new major is required to:

- rename or remove any published method or field;
- remove a deprecated method;
- change a canonical type, wire encoding, requiredness, nullability, default, range, length, or bound incompatibly;
- add an essential state-projection event variant, or add any closed-union variant without negotiated feature/view isolation;
- change an enum value or discriminator;
- change an error code or `data.kind` meaning;
- change envelope, request-ID, params, integer, event, or cursor representation;
- weaken authority, redaction, or transport framing rules; or
- change durability, replay, ordering, fencing, or idempotency semantics incompatibly.

Deprecation is guidance only and never permits same-major removal. A compatibility checker compares the proposed bundle with the last released bundle for the same major and has no waiver flag for breaking changes.

## Generation Workflow

A protocol change follows this order:

1. Update the behavioral spec when semantics change.
2. Edit split OpenRPC/JSON Schema source.
3. Update the Desktop authority manifest if renderer exposure changes.
4. Regenerate the bundle, Rust bindings, complete TypeScript model, safe Desktop bindings, and fixture skeletons.
5. Add canonical and invalid fixtures.
6. Implement Rust handlers and domain projections.
7. Run structural, compatibility, cross-language, transport, security, and product tests.
8. Review source IDL, bundled artifact, generated code, and semantic diff together.

Planned commands:

```text
make rpc-idl-generate
make rpc-idl-check
```

Generation is deterministic. The check command does not modify the worktree and fails on stale output, unsupported schema, unresolved references, missing method/error/notification coverage, compatibility violations, unsafe Desktop exposure, or fixture mismatch.

## Implementation Plan

### Phase 0: behavioral inventory

- Map every current v1 method, error, notification, capability, authorization rule, and durability guarantee to retain, redesign, or drop.
- Identify every current cross-crate type that needs a stable public projection.
- Audit open JSON, paths, secrets, integer widths, defaults, aliases, and trial-deserialized unions.
- Record the intended major-2 method and event catalog before code generation.

### Phase 1: canonical major-2 IDL

- Add `protocol/starweaver-host/v2/` source.
- Define common scalars, initialize, typed errors, events, and representative session/run methods first.
- Produce a self-contained OpenRPC bundle.
- Validate it with the official meta-schema and an independent parser.
- Establish canonical and invalid major-2 fixtures independently of v1.

### Phase 2: generated Rust boundary

- Generate wire types, validators, method metadata, server trait, and dispatcher.
- Implement adapters into current service/domain types.
- Implement initialize, typed errors, decimal scalars, and event replay as the first vertical slice.
- Add stdio and HTTP major dispatch.
- Remove handwritten major-2 registries; v1 code remains isolated if dual support is temporarily required.

### Phase 3: generated TypeScript and Desktop surface

- Generate the complete TypeScript model and cross-language fixture tests.
- Add the reviewed Desktop authority manifest.
- Generate safe bridge DTOs, Rust constructors/projections, and `DesktopHostClient`.
- Implement initialize, session list/get, run start/status, replay, subscribe, event delivery, and cancellation as the Desktop vertical slice.
- Prove renderer import confinement and supervisor-owned field injection resistance.

### Phase 4: cutover and external proof

- Require major 2 for Desktop execution.
- Publish the bundled IDL with releases.
- Build an independent Go or Python conformance client using only the public bundle and profile documentation.
- Verify initialize, session list/get, run start/status, replay, stdio subscription, unary HTTP, typed errors, and reconnect recovery.
- Retire v1 only under explicit release policy; never silently reinterpret v1 frames as major 2.

## Validation Gates

The architecture is not implemented until CI proves:

- split-source lint and deterministic bundling;
- official OpenRPC 1.4.0 meta-schema validation;
- successful loading by a pinned independent OpenRPC parser;
- no unresolved local references or mixed schema dialects;
- exact method, event, feature, transport, scope, and error coverage, including one unique typed `x-starweaver-error-data` mapping per Error Object;
- no implicit `any`, unbounded open object, language-specific type reference, or unsupported schema construct;
- canonical fixtures validate and invalid fixtures fail in both generated Rust and TypeScript;
- Rust and TypeScript encode canonical requests identically;
- Rust and TypeScript decode results, errors, replay records, and notifications equivalently;
- decimal-u64 min/max/canonical-form tests;
- string-only client request-ID, protocol-error `id: null`, and object-required params tests;
- typed initialize feature-negotiation, required-subset, ASCII-ID, and canonical-sort tests;
- typed error-registry uniqueness, discriminator consistency, redaction, and retry/reconciliation tests;
- opaque cursor family/scope/view binding, mismatch non-disclosure, empty-page `nextCursor`, and delivery-sequence recovery tests;
- delivery-sequence fixtures for first-value loss/repetition/reordering, later gaps, writer allocation, and exhaustion without wraparound;
- identical feature/authorization eligibility across replay catch-up and live delivery, including view-profile admission, logical-stream isolation, and authority-change closure;
- atomic state/outbox/event publication crash-point and idempotent recovery tests proving that durable records contain no view-bound cursor;
- replay/live `EventDelivery` schema identity, multi-view cursor materialization over one stable event identity, fence handoff, and cursor recovery tests;
- slow-consumer tests in which ordinary event capacity and terminal-frame delivery both fail, proving reserved control capacity or full transport close;
- unsubscribe response-flush tests proving no later event for that subscription generation;
- same-major compatibility comparison against the previous released bundle;
- generated files are current and formatted;
- `starweaver-rpc` passes in-process, stdio, HTTP, backpressure, malformed-frame, and shutdown-barrier tests;
- Desktop imports only safe generated modules;
- renderer attempts to inject supervisor-owned fields fail closed;
- raw paths, credentials, private diagnostics, and unselected fields cannot cross result or notification projections; and
- an independent non-Rust client interoperates without reading Rust source.

The intended aggregate commands are:

```text
make rpc-idl-check
make rpc-contracts-check
make desktop-boundaries-check
make desktop-check
make fmt-check
make check
make test
git diff --check
```

## Acceptance Criteria

This target is complete when:

01. one reviewed OpenRPC/JSON Schema source defines every major-2 structural contract;
02. a deterministic self-contained bundle ships with Starweaver releases;
03. `starweaver-rpc-core` major-2 wire types, server trait, dispatcher, and validators are generated;
04. `starweaver-rpc` implements the generated boundary without a competing handwritten major-2 registry;
05. request IDs are string-only and all potentially 64-bit JSON values use canonical decimal strings;
06. initialize explicitly negotiates required and supported features;
07. every public error has one generated, typed, redacted machine-readable data schema registered in the bundled IDL;
08. durable replay and live delivery share one canonical `EventRecord`, `EventDelivery`, and `HostEvent` vocabulary;
09. event-required state transitions atomically commit a view-independent event or outbox entry and recover to exactly one semantic replay record;
10. opaque delivery cursors bind family, scope, and event view, while replay and live delivery enforce identical feature/authorization eligibility;
11. Desktop consumes only manifest-filtered safe TypeScript bridge/client bindings;
12. the Rust supervisor retains transport, routing, request identity, idempotency, recovery, wire construction, projection, and authority;
13. same-major breaking changes fail CI;
14. stdio, HTTP, Rust, TypeScript, and Desktop conformance gates pass; and
15. an independent language client can interoperate using only the published IDL and protocol-profile documentation.
