# Security Policy

## Supported Versions

The project is pre-1.0. Security fixes are applied to the `develop` branch and carried into the next release tag.

## Reporting a Vulnerability

Please do **not** open a public GitHub issue for security vulnerabilities.

Report vulnerabilities privately via [GitHub Security Advisories](https://github.com/mumei-lang/mumei/security/advisories/new) ("Report a vulnerability"), or by emailing the maintainer listed on the repository profile.

Please include:

- A description of the issue and its potential impact
- Steps to reproduce or a proof of concept
- Affected versions/commits, if known

We aim to acknowledge reports within 7 days and will coordinate disclosure timing with the reporter.

## Scope Notes

- `mumei` executes generated/native code only through documented user-invoked commands (`build`, `run`, emitter plugins); untrusted `.mm` input is expected to be *verified*, not sandboxed — sandbox gaps are still in scope.
- Proof certificates (`.proof-cert.json`) and remote registry artifacts are trust boundaries: bypasses of certificate verification or registry integrity checks are security issues.
- Secrets handling rules for contributors live in [`AGENTS.md`](AGENTS.md); leaks of credentials into outputs, logs or artifacts are treated as security issues.
