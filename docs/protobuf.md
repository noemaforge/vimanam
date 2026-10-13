# Protocol Buffers and gRPC contracts

Vimanam accepts proto2/proto3 `.proto` source and binary `FileDescriptorSet`
inputs (`.pb`, `.bin`, `.binpb`, `.fds`, `.desc`, `.descriptor`, `.protoset`). Compiling source requires the
**official `protoc` compiler on PATH**; Vimanam invokes it without code generation,
with `--include_imports` and `--include_source_info`. Descriptor inputs work
without protoc or buf. Build complete descriptors using:

```sh
protoc -I proto --include_imports --descriptor_set_out=api.pb proto/api.proto
buf build -o api.pb
```

The pinned Rust reflection library currently supports proto2/proto3 only.
**Editions and unknown syntax are explicitly rejected**, including descriptor
inputs, before any output is created. Vimanam does not relabel Editions as proto2
or proto3: resolved edition features would otherwise be lost. Original descriptor
bytes and reflection options retain unknown/custom options and import metadata.
Imported types and services participate in the surface and comparison.

```sh
vimanam proto/api.proto -I proto --detail full --include-schemas
vimanam api.pb --operation 'RPC example.v1.Service.List' --max-tokens 1600
vimanam api.pb --schema example.v1.ListRequest
vimanam api.pb --output-mode skill --detail full --include-schemas -o context/
vimanam diff old.pb new.pb --format json --report
vimanam diff old.proto new.proto -I proto --fail-on-breaking --breaking wire
vimanam diff --from-ref v1 --to-ref v2 --spec proto/api.proto -I proto --breaking source --fail-on-breaking
```

Operation IDs are fully qualified RPC names. Markdown uses native `RPC`, input
message and `result` labels; messages show field numbers, JSON names, oneofs,
requiredness and presence. Enum descriptions retain value numbers. The existing
selection, schema-depth, budget and protected split/Skill-tree machinery applies.
Use `--operation-id example.v1.Service.List` as an alternative to `--operation`.
Schemas use their fully qualified names, including nested types.

Git source comparisons require an explicit `--spec` (or paired `--from-spec` and
`--to-spec`) because existing OpenAPI automatic candidate discovery is unchanged.
All tracked regular `.proto` files are materialized from the exact selected
commit into a temporary snapshot. Include roots are repository-relative and
cannot escape it; symlink proto entries are rejected. Imports cannot read newer
worktree files. Standard compiler-provided imports remain available. Descriptor
sets at Git refs are decoded directly. Missing imports fail before output creation.

## Identity and compatibility

Neutral contract v2 is the protobuf JSON format; `--contract 1` is OpenAPI-only
and rejects protobuf input before creating output. Symbol IDs use fully qualified
names. Fields retain their containing message plus field number, so a same-number
rename remains a surviving wire identity. Enum value IDs additionally include the
name to preserve distinct `allow_alias` labels. Native locations always retain
full names and numbers. Entities and facts are sorted; comments, source-info,
file order and declaration order do not alter identity or change IDs.
Each descriptor file is represented by a `type` entity with `kind: file` and
its file options. File additions/removals are provenance metadata and do not
act as removed/added message APIs. File-option changes are compared once per
file, including empty and extension-only files; renamed files are paired only
when their declared symbols identify a unique counterpart.

`--breaking wire|source|all` scopes **only** the protobuf failure gate. The full
report always includes every fact. Default `all` fails on any breaking fact;
OpenAPI behavior is unchanged. Source scope includes JSON compatibility impacts.
Changes needing review never trigger exit 3.

| Evidence | Verdict / dimension | Native rule |
|---|---|---|
| Field type changes | Breaking wire, source, JSON | `FIELD_SAME_TYPE` |
| Required field removed | Breaking wire and source even when its number is reserved | `FIELD_REQUIRED_NO_DELETE` |
| Field removed without reserving its number | Breaking wire; removal also breaks source | `FIELD_NO_DELETE_UNLESS_NUMBER_RESERVED`, `FIELD_NO_DELETE` |
| Proto2 group changes to an embedded message | Breaking wire because group tags use wire types 3/4 | `FIELD_SAME_GROUP_ENCODING` |
| Enum value removed without reserving number | Breaking wire; removal also breaks source | `ENUM_VALUE_NO_DELETE_UNLESS_NUMBER_RESERVED`, `ENUM_VALUE_NO_DELETE` |
| Existing enum label reassigned to another used number | Breaking wire | `ENUM_VALUE_NUMBER_REUSED` |
| Previously reserved number/name becomes active | Breaking wire for number; source/JSON for name | `RESERVED_IDENTITY_REUSED` |
| Existing field's oneof membership changes | Conservatively breaking wire/source | `FIELD_SAME_ONEOF` |
| Field renamed at the same number/type | Breaking source/JSON | `FIELD_SAME_NAME` |
| Message/enum/service/RPC removed or renamed | Breaking source | `MESSAGE_NO_DELETE`, `ENUM_NO_DELETE`, `SERVICE_NO_DELETE`, `RPC_NO_DELETE` |
| Package changes | Breaking source | `FILE_SAME_PACKAGE` |
| RPC streaming changes | Breaking source | `RPC_SAME_CLIENT_STREAMING`, `RPC_SAME_SERVER_STREAMING` |
| JSON name changes | Breaking source/JSON | `FIELD_SAME_JSON_NAME` |
| New optional field, type, enum label, RPC | Non-breaking | `PROTOBUF_SURFACE_CHANGED` |
| Effective defaults, deprecation, unknown/custom options | Review | `DefaultChanged`, `DEPRECATED_CHANGED`, `PROTOBUF_SURFACE_CHANGED` |

These native names align with [Buf's rule vocabulary](https://buf.build/docs/breaking/rules/)
where applicable; Vimanam does not claim to implement every Buf category/rule.
Field type changes are conservative, including usage-dependent wire-compatible
conversions. [Protobuf's evolution guidance](https://protobuf.dev/programming-guides/proto3/)
explains the distinction between safe and compatible changes. A schema cannot
prove that a same-type, same-number rename changes semantic meaning: Vimanam
reports a source/JSON break instead of inventing a wire verdict. Alias additions
are non-breaking; alias removals break source, but do not delete the wire number
while another label retains it. Numeric reservation bounds handle the full signed
enum range. Effective defaults are compared after reflection resolves them, so
changing the first enum value can change an implicit proto2 default. Explicit
defaults that resolve to the same value remain stable, and non-finite float
defaults retain typed `NaN`/infinity representations in the neutral surface.
Package namespace entities are represented as `type` entities with
`kind: package`, allowing file-package facts to reference an entity present in
the compared surface.

An RPC with `page_size` and `page_token` request fields and `next_page_token`
response fields has inferred AIP-158 pagination. Gaining/losing this shape emits
`pagination_changed` with a review verdict: names alone do not prove server behavior.
Vimanam provides no runtime gRPC reflection or clients.

The public library surface is `contract::protobuf::{Document, adapt, diff,
documentation, write_diff}` and is available with `default-features = false`.
`Document::decode` does not invoke external programs and retains original bytes.
`documentation` returns the existing renderer IR; its `RPC` and `result` labels
are protobuf-native and should not be interpreted as HTTP methods/statuses.
