---
name: Bug report
about: Report incorrect or unexpected behavior
title: ""
labels: bug
assignees: ""
---

## Description

A clear description of what's wrong.

## To reproduce

The exact command and flags you ran:

```bash
vimanam spec.json --detail full --group-by path ...
```

- **OpenAPI version of the input:** 2.0 (Swagger) / 3.0+ / not sure
- If possible, attach or paste a **minimal spec** that reproduces the issue
  (the smallest input that still shows the problem).

## Expected vs actual

**Expected:** what you thought the output / behavior would be.

**Actual:** what happened instead (paste the relevant Markdown output or error;
running with `RUST_LOG=debug` can add useful detail).

## Environment

- `vimanam --version`:
- Installed via: crates.io / prebuilt binary / built from source
- OS:
