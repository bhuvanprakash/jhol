# Organization bootstrap seed

This directory holds the local bootstrap index and repository-root seed for the
future GitHub organization repository named `.github`:

- `github-repo/profile/README.md` — Jhol-authored organization profile.
- `github-repo/CONTRIBUTING.md` — shared contribution expectations; each repository keeps its own workflow.
- `github-repo/ISSUE_TEMPLATE/` — bug and feature forms with safe-publication reminders.
- `github-repo/PULL_REQUEST_TEMPLATE.md` — focused change, evidence, and side-effect prompts.
- `github-repo/SECURITY.md` — routes reports to Jhol's existing private disclosure policy.

The contents of `github-repo/` map directly to the root of the future `.github`
repository. `organization/README.md` is only this local bootstrap index.

Future repositories have launch READMEs under `repositories/`: `jhol-benchmarks`,
`jhol-examples`, `jhol-site`, and private `jhol-cloud`. The existing checkout
becomes `jhol` through reviewed transfer; it is not copied into a fresh repo.

Keep this seed small. Source code remains in the `jhol` monorepo; split other
repositories only at the release and operating boundaries described in the
[organization blueprint](../docs/architecture/JHOL_ORGANIZATION_ARCHITECTURE_BLUEPRINT.md).

The actual organization handle is not chosen or verified here. Creating the
organization, transferring `bhuvanprakash/jhol`, and changing its remotes need
the owner's GitHub access and the reviewed transfer checklist in the blueprint.

## Repository map

| Repository | Boundary | Seed state |
|---|---|---|
| `.github` | Shared profile, contribution guidance, issue forms, and disclosure policy. | Repository-root seed is under `github-repo/`. |
| `jhol` | Central Rust monorepo for CLI, package manager, graph, adapters, runtime, bundler, and shared schemas. | This checkout is the transfer source; keep its history. |
| `jhol-benchmarks` | Reproducible performance and compatibility evidence. | README, contribution guide, and security policy draft. |
| `jhol-examples` | Versioned, runnable demonstrations of supported workflows. | README, contribution guide, and security policy draft; current example remains in `examples/blueprint/`. |
| `jhol-site` | Onboarding and user-facing product information. | README, contribution guide, and security policy draft. |
| `jhol-cloud` | Optional private cloud control plane and remote-cache services. | README, contribution guide, and security policy draft; no service implementation exists yet. |

The organization repos are release and operating boundaries. They are not a
request to split core subsystems into separate repositories. Public visibility,
access control, secrets, branch rules, and issue routing are set only after the
organization owner selects a handle and reviews the disclosure boundary.
