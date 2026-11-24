# Public protobuf fixture provenance

These are unchanged gRPC health service definitions from two published releases:

- [`health-v1.0.0.proto`](https://github.com/grpc/grpc/blob/v1.0.0/src/proto/grpc/health/v1/health.proto), gRPC v1.0.0. Original Google BSD-style license notice retained in the file.
- [`health-v1.20.0.proto`](https://github.com/grpc/grpc/blob/v1.20.0/src/proto/grpc/health/v1/health.proto), gRPC v1.20.0. Original Apache-2.0 notice retained in the file.

The corresponding `.pb` files are complete binary FileDescriptorSets generated
with official protoc 36.2, retaining source filenames and options, excluding
source-info comments. Regenerate from the repository root:

```sh
protoc -I tests/fixtures/protobuf --include_imports --descriptor_set_out=tests/fixtures/protobuf/health-v1.0.0.pb tests/fixtures/protobuf/health-v1.0.0.proto
protoc -I tests/fixtures/protobuf --include_imports --descriptor_set_out=tests/fixtures/protobuf/health-v1.20.0.pb tests/fixtures/protobuf/health-v1.20.0.proto
```

`custom-options.proto` is synthetic: a custom `FieldOptions` extension (importing
`google/protobuf/descriptor.proto`), a source-retention option that protoc strips
from descriptors, and a `google.protobuf.Timestamp` field.
`custom-options.pb` embeds protoc 36.2's `descriptor.proto` (Editions `FeatureSet`
and newer option fields), which differs from the copy protox bundles; mixed
`.pb`/`.proto` diffs of it must report no changes. Regenerate from the
repository root:

```sh
protoc -I tests/fixtures/protobuf --include_imports --descriptor_set_out=tests/fixtures/protobuf/custom-options.pb tests/fixtures/protobuf/custom-options.proto
```

`health-expected.json` records the neutral additions (`Watch` server-streaming
RPC and `SERVICE_UNKNOWN` enum value), plus four native file-option changes (Go/Java/Objective-C generated-code settings).
Binary/source input parity is tested independently of source-info and licenses.
Synthetic inline fixtures in the Rust tests isolate each compatibility rule;
they are not copied from an external schema.
