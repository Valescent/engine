# Valescent Engine documentation

Source of the documentation site and the folder index on GitHub.

The site is built with mdBook. `book.toml` configures it, `SUMMARY.md` defines the navigation, and a page that is not listed there does not appear on the site. Push to `main` to deploy, or run `mdbook build` locally.

## Layout

| Path | Contents |
|---|---|
| `index.md` | Landing page for the site. |
| `roadmap.md` | Versioned milestones and their status. |
| `dev/` | Development setup, conventions, architecture, testing, release process, milestone scopes, and templates. |
| `decisions/` | Architecture decision records, indexed in [`decisions/index.md`](decisions/index.md). |
| `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`, `SECURITY.md`, `CLA.md` | Project policy. |

Add new top-level folders only when a real document needs them, for example `guides/` for how-to material or `specs/` for formats and protocols. Do not create empty placeholders.

## Conventions

- **Links are relative.** Use `dev/setup.md`, not `/docs/dev/setup.md`, so the same link works on GitHub and in the book.
- **One title per page.** The `#` heading is the title; do not repeat the page name in a subtitle.
- **Roadmap stays flat.** Milestones live in one `roadmap.md`; per-version files are not worth the navigation cost until the file becomes unmanageable. A scope page may expand one milestone's deliverables, but the roadmap remains the status source of truth.
