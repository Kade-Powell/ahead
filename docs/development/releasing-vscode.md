# Releasing the VS Code Extension

Audience: AHEAD framework and tooling maintainers

## Contract

- Marketplace extension: `ahead-framework.ahead-vscode`
- release workflow: `.github/workflows/publish-vscode.yml`
- release tag: `vscode-v<package-version>`
- GitHub environment: `vscode-marketplace`

Publishing starts only from a published GitHub Release. The workflow requires the tagged commit to be on `main`, matches the tag to `integrations/vscode/package.json`, rebuilds and tests the extension, attaches the exact VSIX to the GitHub Release, publishes that VSIX to Visual Studio Marketplace, and verifies the version is visible.

## One-time Marketplace setup

1. Create or select the `ahead-framework` publisher in the [Visual Studio Marketplace management portal](https://marketplace.visualstudio.com/manage/publishers/). If a different publisher ID is chosen, update `integrations/vscode/package.json` before the first release.
2. Create a Microsoft Entra application or user-assigned managed identity and add its identity to the Marketplace publisher with the Contributor role.
3. Add a federated credential that trusts this repository's `vscode-marketplace` GitHub environment.
4. Create the `vscode-marketplace` GitHub environment. Add `AZURE_CLIENT_ID` and `AZURE_TENANT_ID` as environment variables and, if desired, require a reviewer.

The workflow uses GitHub OIDC through `azure/login` and `vsce publish --azure-credential`; it stores no Marketplace PAT.

## Publish

From `integrations/vscode`:

```sh
npm version --no-git-tag-version <version>
npm test
node ./scripts/check-release.mjs vscode-v<version>
```

Commit the version, merge it to `main`, confirm CI, then publish a GitHub Release tagged `vscode-v<version>`. An existing Marketplace version is immutable; correct a bad release with a new patch version.
