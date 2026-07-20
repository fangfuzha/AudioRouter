---
name: "release-version-bumper"
description: "Ensures internal version string in Cargo.toml is bumped before creating a GitHub release. Invoke when user asks to publish/release a new version, update release, or create a tag."
---

# Release Version Bumper

This skill ensures the internal version string is always updated before publishing a new release.

## When to Invoke

Invoke this skill IMMEDIATELY when:
- User asks to "发布新版本" / "publish release" / "update release" / "创建 release"
- User asks to commit, push, and create a new version tag
- User mentions creating a GitHub Release or pushing a new tag

## Mandatory Steps

Before creating any git tag or GitHub Release, you MUST:

1. **Check current version**: Read `winui3_gui/Cargo.toml` and find the `version` field in `[package]` section.
2. **Check latest released version**: Run `gh release list --limit 5` or `git tag --sort=-creatordate` to find the latest published version.
3. **Bump version**: Increment the version number in `Cargo.toml`:
   - If Cargo.toml version is BEHIND the latest tag (e.g., Cargo.toml says 0.3.5 but v0.3.6 was already released), bump to the NEXT patch version (0.3.7).
   - If Cargo.toml version MATCHES the latest tag, bump to the next patch version.
   - Only bump patch version unless user specifies minor/major bump.
4. **Verify consistency**: Ensure the Cargo.toml version is NEWER than the latest released tag before committing.
5. **Include version bump in commit**: The version change must be part of the commit before tagging.

## Common Pitfall

A frequent mistake is forgetting to update `Cargo.toml` version before tagging, causing:
- The "check for updates" feature to report wrong version
- The released binary shows an outdated version number in UI
- GitHub release tag (e.g., v0.3.7) doesn't match the binary's internal version (e.g., 0.3.5)

## Example Flow

```
User: "提交推送并发布新版本"
→ Read Cargo.toml (version = "0.3.5")
→ Check gh release list (latest = v0.3.6)
→ Bump Cargo.toml to "0.3.7"
→ Commit all changes (including version bump)
→ Push to main
→ Create tag v0.3.7
→ Push tag
→ Create GitHub Release
```
