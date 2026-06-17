# Releasing scrt

scrt uses two GitHub Actions workflows:

| Workflow | Trigger | What it does |
| :--- | :--- | :--- |
| [`ci.yml`](./.github/workflows/ci.yml) | push to `main`, PRs | `cargo fmt --check`, `clippy -D warnings`, `cargo test` |
| [`release.yml`](./.github/workflows/release.yml) | push a `v*` tag | re-runs the test+clippy gate, then cross-builds binaries and publishes a GitHub Release |

## Cutting a release

A plain push to `main` does **not** release — it only runs CI. You cut a
release by pushing a version tag:

```bash
# 1. make sure main is green (CI passed) and the version is bumped
#    in the workspace Cargo.toml ([workspace.package] version = "x.y.z")

# 2. tag and push
git tag v0.1.0
git push origin v0.1.0
```

That's it. The release workflow then:

1. **Gates** — runs `clippy -D warnings` + `cargo test`. If either fails,
   no release is published.
2. **Builds** the `scrt` binary for all targets (in parallel):
   - `x86_64-unknown-linux-gnu`
   - `aarch64-unknown-linux-gnu` (cross-compiled via [`cross`])
   - `x86_64-apple-darwin` (Intel macOS)
   - `aarch64-apple-darwin` (Apple Silicon)
   - `x86_64-pc-windows-msvc` (`scrt.exe`)
3. **Packages** each as `scrt-<tag>-<target>.{tar.gz,zip}` (binary + README +
   LICENSE) and **uploads** it to the GitHub Release for the tag.
4. **Generates release notes** from the commit history automatically.

Tags with a pre-release suffix (e.g. `v0.1.0-rc.1`) are published as
**prereleases**.

## Downloading a release

Users grab the archive for their platform from the repo's **Releases** page,
unpack it, and put `scrt` (or `scrt.exe`) on their `PATH`. No Node, no
Python, no `ripgrep` — it's a single static-ish binary.

## Versioning

The single source of truth is `[workspace.package] version` in the root
`Cargo.toml`; all crates inherit it via `version.workspace = true`. Bump it
in the same commit (or the commit before) the tag points at, and keep the
tag (`vX.Y.Z`) and the Cargo version (`X.Y.Z`) in sync.

[`cross`]: https://github.com/cross-rs/cross
