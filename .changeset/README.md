# Changesets

One file per user-visible change. On release they become the GitHub release notes and the
changelog on sdrmm.com.

```sh
cargo xtask changeset minor "Airspy HF+: add preamp control"
```

```md
---
bump: minor
---

Airspy HF+: add preamp control.
```

`bump` is `patch` for fixes, `minor` for features, `major` for breaking changes. Write for users,
not for reviewers.
