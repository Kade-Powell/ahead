# Releasing the Pi Extension

Status: publishing path v0.1

## Contract

- npm package: `ahead-pi`
- package source: `integrations/pi`
- release workflow: `.github/workflows/publish-pi.yml`
- release tag: `pi-v<package-version>`
- stable npm dist-tag: `latest`
- prerelease npm dist-tag: `next`
- registry: `https://registry.npmjs.org`

The package name was unclaimed when this path was created. Availability is not ownership until the first version is published.

## Human release gate

Publishing is triggered only by publishing a GitHub Release whose tag starts with `pi-v`. The workflow rejects a tag that does not exactly match `integrations/pi/package.json`, rejects a release commit that is not contained in `main`, rebuilds from source, runs the complete test and packed-install smoke suite, and then publishes with npm provenance.

The job uses the GitHub `npm` environment. Configure that environment with a required reviewer if releases need an explicit second confirmation.

AHEAD is released under the MIT License. The repository license file and each published package must declare the same license.

## Prepare a version

From `integrations/pi`:

```sh
npm version --no-git-tag-version <version>
npm test
node ./scripts/check-release.mjs pi-v<version>
```

Commit the version change, merge it to `main`, and confirm CI. Do not create the release from an unmerged commit.

## Bootstrap the first npm release

This machine was not authenticated to npm when the workflow was created, and npm trusted publishing is configured from an existing package's settings. The first publication therefore needs a short-lived bootstrap credential:

1. Create or select an npm account with 2FA enabled.
2. Create a granular npm access token that can publish the new public package and is permitted to bypass 2FA for automation.
3. Store it as `NPM_TOKEN` in the GitHub `npm` environment. Do not put it in repository files, shell history, workflow logs, or release notes.
4. On GitHub, create and publish a release with tag `pi-v0.1.0` at the corresponding `main` commit.
5. Verify the workflow, npm package page, provenance, and a clean `pi -e npm:ahead-pi@0.1.0` install.

The workflow has `id-token: write` and publishes with `--provenance`. Before trusted publishing exists, npm uses the bootstrap token and GitHub OIDC supplies the provenance attestation.

## Move to tokenless trusted publishing

After the first package exists, open the `ahead-pi` package settings on npm and configure this trusted publisher:

| npm field | Value |
|---|---|
| Provider | GitHub Actions |
| Organization or user | `Kade-Powell` |
| Repository | `ahead` |
| Workflow filename | `publish-pi.yml` |
| Environment | `npm` |
| Allowed action | `npm publish` |

Publish the next version and verify that OIDC authentication and automatic provenance succeed. Then remove the `NPM_TOKEN` environment secret, revoke the bootstrap token, and configure npm publishing access to require 2FA while disallowing traditional tokens. Keep the trusted publisher.

## Publish and verify

1. Publish a GitHub Release with tag `pi-v<version>` at the version commit.
2. Watch the `Publish Pi extension` workflow.
3. Confirm `npm view ahead-pi@<version> version` returns the release.
4. Confirm the package page links to this repository and shows provenance.
5. Test the registry artifact:

   ```sh
   pi -e npm:ahead-pi@<version>
   ```

6. For stable releases, confirm `npm view ahead-pi dist-tags.latest`; for prereleases, confirm `dist-tags.next`.

## Failure and rollback

An npm version is immutable. Do not reuse a version after any publish attempt.

- If validation fails before `npm publish`, fix the source, increment the version if a registry publish may have occurred, and publish a new GitHub Release.
- If publication succeeds but post-publish verification fails, do not rerun the same version blindly. Inspect the registry first.
- Correct a bad release with a new patch version. Use npm deprecation for a discoverable warning; reserve unpublishing for the narrow cases allowed by npm policy.
- Moving a dist-tag is a human release decision and does not alter the immutable tarball.

## Why npm and OIDC

Pi natively installs `npm:ahead-pi`, pinned npm versions, Git sources, and local paths. npm provides the simplest cross-machine install path. GitHub-hosted OIDC avoids a long-lived release secret after bootstrap, and npm provenance links the public tarball to this repository and workflow. Neither provenance nor a passing workflow proves the package is safe; they make origin and build history auditable.
