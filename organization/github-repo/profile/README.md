# Jhol

**One command. Every language. Every dependency. Every environment.**

Jhol is a local-first development engine written in Rust. It starts with the
work it knows deeply: JavaScript package installation, npm-compatible
lockfiles, a content-addressed cache, and installs that can work offline when
their artifacts are already present.

Jhol is growing beyond JavaScript through a project graph and adapters. Each
ecosystem keeps its own package manager, version rules, manifests, and lockfile.
Jhol connects those native results with tasks, environments, databases, and
verified artifacts. It does not pretend one resolver can replace every
ecosystem.

## How we build

- **Local first.** Core workflows should remain useful without an account or a
  hosted service.
- **Correctness before speed claims.** A measured install counts only when its
  output is checked. Publish the setup, workload, and raw evidence with results.
- **Use native contracts.** Preserve ecosystem formats and semantics; add a
  Jhol layer only where coordinating them solves a real problem.
- **Make side effects visible.** Package scripts, database changes, process
  launches, and remote uploads need clear boundaries and evidence.
- **Build from Jhol's own work.** Compatibility with established formats is
  useful; copying another project's identity, implementation, or claims is
  not our direction.

The product is evolving. “Every language” describes the direction, not a claim
that Jhol already implements every ecosystem or supports every runtime.

## Start here

- [Main repository](https://github.com/bhuvanprakash/jhol)
- [Getting started](https://github.com/bhuvanprakash/jhol/blob/main/GET_STARTED.md)
- [User documentation](https://github.com/bhuvanprakash/jhol/tree/main/docs/user)
- [Architecture and organization blueprint](https://github.com/bhuvanprakash/jhol/blob/main/docs/architecture/JHOL_ORGANIZATION_ARCHITECTURE_BLUEPRINT.md)
