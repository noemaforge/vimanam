# Public GraphQL fixture provenance

The SDL files are the unchanged public `schema.graphql` of the
[SpaceX Land API](https://github.com/SpaceXLand/api) (MIT License,
Copyright (c) 2019 SpaceX Land) at two published commits:

- [`spacex-land-64a7fa9.graphql`](https://github.com/SpaceXLand/api/blob/64a7fa987ed2ba5f52c7969bbdac5414e986b995/schema.graphql),
  commit `64a7fa987ed2ba5f52c7969bbdac5414e986b995` (2019-02-08, "add launchesPastResult").
- [`spacex-land-0a482d1.graphql`](https://github.com/SpaceXLand/api/blob/0a482d1ff6ffd16a9f345cdc4864b9d9da9d10be/schema.graphql),
  commit `0a482d1ff6ffd16a9f345cdc4864b9d9da9d10be` (2019-02-28, "add historiesResult").

`spacex-land-expected.json` is the checked-in expected change list, verified by
hand against the textual diff of the two files: `Result.count` was removed
(breaking), and four root query fields (`historiesResult`, `missionsResult`,
`rocketsResult`, `shipsResult`) were added with their four result types
(non-breaking).

The `*.introspection.json` files are introspection results of the same SDL,
generated with graphql-js 16.11.0 (`buildSchema` then
`introspectionFromSchema` with `inputValueDeprecation: true`), compact JSON
wrapped in `{"data": …}`:

```js
import { buildSchema, introspectionFromSchema } from 'graphql';
import { readFileSync } from 'fs';
const sdl = readFileSync(process.argv[2], 'utf8');
process.stdout.write(JSON.stringify({ data: introspectionFromSchema(buildSchema(sdl), { inputValueDeprecation: true }) }) + '\n');
```

Tests check SDL/introspection parity at each version and that both inputs
produce the expected list. Synthetic inline schemas in the Rust tests isolate
each compatibility rule; they are not copied from an external schema.
