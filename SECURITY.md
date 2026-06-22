# Security Policy

## Supported versions

Vimanam is pre-1.0 and ships fixes only against the latest release. Security
fixes land on `main` and go out in the next version.

| Version | Supported |
| ------- | --------- |
| latest release (`0.5.x`) | ✅ |
| older releases | ❌ |

## Reporting a vulnerability

Please report security issues **privately** — do not open a public issue for a
suspected vulnerability.

Use GitHub's [private vulnerability reporting](https://docs.github.com/en/code-security/security-advisories/guidance-on-reporting-and-writing-information-about-vulnerabilities/privately-reporting-a-security-vulnerability):
go to the repository's **Security** tab → **Report a vulnerability**. This opens
a private advisory visible only to the maintainers.

When reporting, please include:

- the version (`vimanam --version`) and platform,
- a description of the issue and its impact,
- a minimal spec or input that reproduces it, if applicable,
- any suggested remediation.

You can expect an initial acknowledgement within a few days. Because Vimanam is
a small, single-maintainer project, please allow reasonable time for a fix
before any public disclosure.

## Scope notes

Vimanam reads an OpenAPI/Swagger document and writes Markdown. The most relevant
classes of issue are input-handling problems triggered by a malicious or
malformed spec (for example: excessive resource use, panics, or path/`$ref`
handling that escapes expectations). Reports that demonstrate such behavior from
a crafted input are in scope.
