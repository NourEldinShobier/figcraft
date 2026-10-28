# Contributing

Thanks for helping. Short version:

1. Build and test: `cargo test --release` (CI also runs `cargo xtask ci`: formatting, clippy, layer checks, wasm build).
2. Keep crate layering intact: `cargo xtask layers` fails on an upward dependency.
3. Add a test with literal expected values for behaviour you change.
4. Use conventional commit messages (`fix:`, `feat:`, `docs:`).
5. Contributions are licensed under MIT OR Apache-2.0, like the rest of the project.

Do not paste code from other design tools or from sources with incompatible licences. Learning from how
other tools behave is fine; copying their code is not. Third-party assets go in `ATTRIBUTION.md`.
