# API Diff: Widgets API 1.0.0 → 1.1.0

**Summary:** 1 endpoint added, 1 removed, 4 changed; 4 breaking, 4 non-breaking, 1 to review

## Breaking changes (4)

| Change | Endpoint | Detail |
|--------|----------|--------|
| Parameter newly required | `GET /widgets/{id}` | `fields` (query) |
| Response removed | `GET /widgets/{id}` | 404 |
| operationId changed | `DELETE /widgets/{id}` | `Widgets_DeleteWidget` → `Widgets_RemoveWidget` |
| Endpoint removed | `GET /legacy` | was deprecated |

## Non-breaking changes (4)

| Change | Endpoint | Detail |
|--------|----------|--------|
| Marked deprecated | `DELETE /widgets/{id}` | - |
| Endpoint added | `GET /widgets/{id}/history` | - |
| Component schema changed | component `Widget` | response `/properties/pricing` added (3 operations) |
| Component schema changed | component `Widget` | response `/properties/pricing` added to `required` (3 operations) |

## Needs review (1)

| Change | Endpoint | Detail |
|--------|----------|--------|
| Component schema changed | component `WidgetInput` | request `/properties/weight/format` changed `float` → `double` (1 operation) |

## Affected operations

### `Widget`

- `GET /widgets` response 200
- `POST /widgets` response 201
- `GET /widgets/{id}` response 200

### `WidgetInput`

- `POST /widgets` request body
