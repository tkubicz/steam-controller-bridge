# Contributing

## Pull-request titles

This repository squash-merges pull requests. The pull-request title becomes the
commit consumed by Release Please, so it must use Conventional Commit syntax:

```text
type(scope)!: short summary
```

The scope and breaking-change `!` are optional. Supported types are `feat`,
`fix`, `perf`, `deps`, `refactor`, `docs`, `test`, `build`, `ci`, `chore`, and
`revert`. Use `feat` for a SemVer minor change and `fix`, `perf`, or `deps` for a
patch. Add `!` or a `BREAKING CHANGE:` footer for a SemVer major change.
Internal documentation, test, build, CI, chore, and refactoring entries are
hidden from release notes and do not create a release by themselves.

When one squash merge contains multiple user-visible changes, add this block to
the pull-request description before merging:

```text
BEGIN_COMMIT_OVERRIDE
feat: add configurable controller idle shutdown

fix(menu): reuse native status images to bound memory usage
END_COMMIT_OVERRIDE
```

Release Please uses that block in place of the squash title. Correct inaccurate
notes in the merged pull-request metadata and rerun Release Please; do not edit
`CHANGELOG.md` or the GitHub Release independently.

## Releasing

Release Please runs after changes land on `main` and maintains a release pull
request containing the generated `CHANGELOG.md`, Cargo workspace version,
`Cargo.lock`, and version manifest. To publish a stable release:

The repository is one versioned product: every member inherits
`workspace.package.version` through `version.workspace = true`. The root Rust
release strategy discovers and updates all members and the lockfile. Do not add
Release Please's `cargo-workspace` plugin; it expects literal member versions
and rejects Cargo's inherited-version syntax.

1. Review the generated notes and version in the Release Please pull request.
2. Fix source pull-request metadata if an entry is inaccurate, then rerun the
   Release workflow and review the regenerated result.
3. Merge the Release Please pull request.
4. Wait for the Release workflow to validate and build the tagged source. It
   uploads firmware, the macOS app, and checksums to a draft release, then makes
   it public only after all jobs succeed.

If packaging or publication fails after the draft is created, use **Re-run
failed jobs** on that workflow run. The successful Release Please job and its
release outputs are retained, while artifact uploads safely replace files with
the same names.

Do not manually edit the generated changelog, create a version tag, or compose a
second GitHub Release description. Automated prereleases are not supported yet.

Repository administrators must enable **Allow GitHub Actions to create and
approve pull requests**. The workflow uses the built-in `GITHUB_TOKEN`; it
explicitly dispatches CI for the generated release branch, so no personal token
or long-lived release credential is required.
