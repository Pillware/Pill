# Contributing to Pill

Pill is a novel Rust game engine. Engine code, examples, tests, docs, demo visuals and bug reports all help. If you want extra credit, showcase what you did with Pill!

## Find a task

- Check the [issues](https://github.com/Pillware/Pill/issues). New here? Ask for a small, self-contained issue.
- Bug fix? Include a minimal reproduction and expected behavior.
- Feature or public API change? Discuss the approach in an issue before coding.
- Website and documentation changes belong in [Pill-Website](https://github.com/Pillware/Pill-Website).

Need a pointer? Find us on [Discord](https://discord.gg/VUKNQrctms).

## Build and run

The [setup guide](https://docs.pillengine.org/guide/getting-started/setup) covers platform dependencies. From the repository root:

```sh
cargo build --release --manifest-path engine/pill_launcher/Cargo.toml
./engine/pill_launcher/target/release/PillLauncher -a run -p examples/cube
```

On Windows, use `engine\pill_launcher\target\release\PillLauncher.exe` instead.

## Before opening a PR

- Keep it focused. Add a regression test or example where useful.
- Follow the [coding standards](https://docs.pillengine.org/guide/contributing/coding-standards).
- Run the relevant example/tests. These are the Rust checks used by CI:

```sh
./engine/pill_launcher/target/release/PillLauncher -a cargo -p examples/floating_pills -- fmt
./engine/pill_launcher/target/release/PillLauncher -a cargo -p examples/floating_pills -- clippy -- -D warnings
```

- Explain **why**, link an issue if there is one, and list what you tested. Include a screenshot or short clip for visible changes.
- Expect review feedback. Small PRs are easier to review and merge.

Contributions are credited in Git history; we may also feature contributors in project updates.

Follow the [code of conduct](https://docs.pillengine.org/guide/contributing/code-of-conduct). Report security-sensitive issues privately to a maintainer, not in public issues.

Pill is dual-licensed under [MIT](LICENSE-MIT) and [Apache-2.0](LICENSE-APACHE).
