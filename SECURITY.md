# Security Policy

Valescent Engine is pre-alpha. There are no supported releases; fixes land on `main`.

## Reporting a vulnerability

Do not open a public issue.

Use GitHub's private vulnerability reporting: **Security → Report a vulnerability** on this repository. The report is visible only to maintainers.

Include, as far as you can:

- The affected commit or version.
- What the flaw allows an attacker to do.
- Steps to reproduce, or a proof of concept.
- Your operating system, GPU, and driver, if the issue is platform-specific.

You get an acknowledgement when the report is triaged. Disclosure timing is agreed with you before anything is published.

## Scope

In scope: engine code in `src/`, build and release workflows, and the shipped binaries once there are any.

Out of scope: vulnerabilities in third-party dependencies and GPU drivers. Report those upstream, then tell us if the engine needs a workaround.
