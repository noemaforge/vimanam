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

`health-expected.json` records the neutral additions (`Watch` server-streaming
RPC and `SERVICE_UNKNOWN` enum value), plus four native file-option changes (Go/Java/Objective-C generated-code settings).
Binary/source input parity is tested independently of source-info and licenses.
Synthetic inline fixtures in the Rust tests isolate each compatibility rule;
they are not copied from an external schema.
