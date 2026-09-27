# Security Policy

## Supported versions

Highland is pre-1.0. Security fixes are provided for the latest released
version and for the development branch.

| Version | Supported |
|---|---|
| 0.1.x | yes |
| < 0.1 | no |

## Reporting a vulnerability

Report privately through the repository's security advisory form, not as a
public issue. Include:

- the Highland version and commit
- the configuration involved, with secrets removed
- what an attacker gains
- a reproduction, if you have one

You can expect an acknowledgement within a few days and an assessment within two
weeks. Fixes are released as a patch version and credited in `CHANGELOG.md`
unless you ask otherwise.

## Threat model

The full threat model lives in [`docs/threat-model.md`](docs/threat-model.md). In
summary:

- All network input is untrusted. Parsers MUST NOT panic and every size is
  bounded.
- The control API is a local Unix socket only. It MUST NOT be exposed over a
  network transport, and the socket MUST NOT be world-writable.
- Command execution is disabled by default, behind a Cargo feature, and
  restricted to an explicit allow-list of absolute paths.
- World-writable configuration files are refused.
- The daemon needs `CAP_NET_ADMIN` and `CAP_NET_RAW`. It does not need
  unrestricted root.

## Out of scope

- Denial of service from a network position that can already flood the host.
- Anything requiring an attacker to already control the configuration file or the
  host.
- Split-brain conditions. VRRP cannot prevent every partition, and Highland does
  not claim to; see [`docs/operations.md`](docs/operations.md).
