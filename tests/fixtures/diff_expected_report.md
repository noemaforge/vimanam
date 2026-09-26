# API Diff: Widgets API 1.0.0 → 1.1.0

**Summary:** 1 endpoint added, 1 removed, 4 changed; 4 breaking, 8 non-breaking, 1 to review

## Breaking changes (4)

| Change | Endpoint | Detail |
|--------|----------|--------|
| Parameter newly required | `GET /widgets/{id}` | `fields` (query) |
| Response removed | `GET /widgets/{id}` | 404 |
| operationId changed | `DELETE /widgets/{id}` | `Widgets_DeleteWidget` → `Widgets_RemoveWidget` |
| Endpoint removed | `GET /legacy` | was deprecated |

## Non-breaking changes (8)

| Change | Endpoint | Detail |
|--------|----------|--------|
| Response schema changed | `GET /widgets` | 200 `/properties/pricing` added |
| Response schema changed | `GET /widgets` | 200 `/properties/pricing` added to `required` |
| Response schema changed | `POST /widgets` | 201 `/properties/pricing` added |
| Response schema changed | `POST /widgets` | 201 `/properties/pricing` added to `required` |
| Response schema changed | `GET /widgets/{id}` | 200 `/properties/pricing` added |
| Response schema changed | `GET /widgets/{id}` | 200 `/properties/pricing` added to `required` |
| Marked deprecated | `DELETE /widgets/{id}` | - |
| Endpoint added | `GET /widgets/{id}/history` | - |

## Needs review (1)

| Change | Endpoint | Detail |
|--------|----------|--------|
| Request schema changed | `POST /widgets` | `/properties/weight/format` changed `float` → `double` |

## Deltas

| Check | Old | New | Δ |
|-------|----:|----:|--:|
| Missing description | 0 | 1 | +1 |
| Missing operationId | 0 | 0 | 0 |
| No responses documented | 0 | 0 | 0 |
| Deprecated | 1 | 1 | 0 |
| Untagged (no service tag) | 0 | 0 | 0 |
| Duplicate operationIds | 0 | 0 | 0 |
| Parameters without description | 0 | 0 | 0 |

Token estimate (--detail full --include-schemas): 993 → 1140 (+147)
