# Developer documentation

Orpheus is two Cargo workspaces, a desktop simulator and one tool,
`cargo xtask`, for everything from building to talking to the player.

| Start with | For |
|---|---|
| [Getting started](getting-started.md) | Toolchain, simulator, golden images, the gate, a first change, working with the player safely |
| [Architecture](architecture.md) | Memory, boot, demand paging, executors, crates, on one page |
| [How-to guides](how-to/README.md) | Add a screen, a setting, a codec, a driver, a test command, a fuzz target; profile; read a fault record |
| [Contributing](../../CONTRIBUTING.md) | How changes are proposed and reviewed |

The plan and the log are the deeper sources; these pages link to them
rather than repeat them:

* [PLAN.md](../PLAN.md): scope, decisions, decisions still open.
* [The plan documents](../plan/01-hardware.md) (01 hardware … 09 docs and
  website), in particular [07 tooling and testing](../plan/07-tooling-testing.md)
  and [08 milestones](../plan/08-milestones.md).
* [STATUS.md](../STATUS.md): what is done, measured numbers, open items.
* [USER-CHECKS.md](../USER-CHECKS.md): checks that need a person.

API documentation (rustdoc) for every crate will be published with the
website; until then `cargo doc --workspace --no-deps --open`.
