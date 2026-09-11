Git-Xet is a Git LFS custom transfer agent for native Xet uploads and downloads.

## This fork

This fork implements the missing download handler and adds standalone LFS negotiation, so native downloads work with Hugging Face's current servers. One executable handles uploads, parallel chunk downloads, token refresh, and verification. No `hf-xet`, `huggingface-hub`, or Python transfer script is required at runtime. Git and Git LFS are still required.

Download a platform archive or a native executable wheel from this fork's [releases](https://github.com/haraschax/xet-core/releases). Wheels can be installed with `uv pip install /path/to/git_xet-*.whl`; they contain the executable, with no Python library dependencies. The archive installer uses `curl`, `tar`, and the platform's SHA-256 utility, without requiring `unzip` or root access:

```sh
curl -fsSL https://raw.githubusercontent.com/haraschax/xet-core/git-xet-downloads/git_xet/install.sh | sh
```

Configure each repository with its HTTP(S) **LFS endpoint**:

```sh
git xet install --local --lfs-url https://huggingface.co/OWNER/REPO.git/info/lfs
git lfs pull
git push
```

For a dataset, include `/datasets/OWNER/REPO.git/info/lfs`. The endpoint may differ from the Git remote, as in openpilot. The standalone agent handles both directions, including on older Git LFS versions, and `git xet uninstall --local` removes its configuration. Use one LFS endpoint per local standalone configuration; uploads to servers that do not support Xet should keep using ordinary Git LFS.

The agent requests LFS object metadata, extracts the Xet hash from the bridge URL, obtains a scoped read token, and calls the existing Rust reconstruction engine. Plain LFS objects retain their HTTP download path. Both paths check the expected size and SHA-256; failed downloads are removed. Public downloads do not prompt for credentials. Private requests use the existing Git/HF credential machinery after an authentication challenge. The read-token route currently uses the Hub's `main` revision, including for LFS objects with no associated Hub file commit.

`git xet --verbose --log /path/to/xet.log transfer --lfs-url URL` enables diagnostics; stdout remains reserved for the Git LFS protocol.

Build from source with Rust 1.95:

```sh
cargo build --locked --release -p git_xet
cargo test --locked -p git_xet --features simulation --lib
cd git_xet
uv build --wheel
```

OpenSSL is vendored into the executable by default. Release packages strip debug symbols. Linux release archives require glibc 2.35 or newer; comma's dependency builds compile from the pinned source for their manylinux baseline. macOS and Windows fork binaries are unsigned.

## Upstream installation

The upstream packages below do not contain this fork's download implementation.

## Installation
### Prerequisite
Make sure you have [git](https://git-scm.com/downloads) and [git-lfs](https://git-lfs.com/) installed and configured correctly.
### macOS or Linux (amd64 or aarch64)
 To install using Homebrew:
   ```
   brew install git-xet
   git xet install
   ```
 Or, using an installation script, run the following in your terminal (requires `curl` and `unzip`):
   ```
   curl --proto '=https' --tlsv1.2 -sSf https://raw.githubusercontent.com/huggingface/xet-core/refs/heads/main/git_xet/install.sh | sh
   ```
  To verify the installation, run:
   ```
   git xet --version
   ```

### Windows (amd64)
 Using `winget`:
 ```
 winget install git-xet
 ```

 Using an installer: 
 - Download `git-xet-windows-installer-x86_64.zip` ([available here](https://github.com/huggingface/xet-core/releases/download/git-xet-v0.2.0/git-xet-windows-installer-x86_64.zip)) and unzip. 
 - Run the `msi` installer file and follow the prompts.
   
 Manual installation:
 - Download `git-xet-windows-x86_64.zip` ([available here](https://github.com/huggingface/xet-core/releases/download/git-xet-v0.2.0/git-xet-windows-x86_64.zip)) and unzip. 
 - Place the extracted `git-xet.exe` under a `PATH` directory.
 - Run `git-xet install` in a terminal.

To verify the installation, run:
  ```
  git xet --version
  ```

## Uninstall
### macOS or Linux
Using Homebrew:
   ```
   git xet uninstall
   brew uninstall git-xet
   ```
If you used the installation script (for MacOS or Linux), run the following in your terminal:
   ```
   git xet uninstall
   sudo rm $(which git-xet)
   ```
### Windows
If you used `winget`:
```
winget uninstall git-xet
```

If you used the installer:
- Navigate to Settings -> Apps -> Installed apps
- Find "Git-Xet".
- Select the "Uninstall" option available in the context menu.

If you manually installed:
- Run `git xet uninstall` in a terminal. 
- Delete the `git-xet.exe` file from the location where it was originally placed.

## How It Works
Git-Xet works by registering itself as a custom transfer agent to Git LFS by name "xet". On `git push`, `git fetch` or `git pull`, `git-lfs` negotiates with the remote server to determine the transfer agent to use. During this process, `git-lfs` sends to the server all locally registered agent names in the Batch API request, and the server replies with exactly one agent name in the response. Should "xet" be picked, `git-lfs` delegates the uploading or downloading operation to `git-xet` through a sequential protocol.

For more details, see the Git LFS [Batch API](https://github.com/git-lfs/git-lfs/blob/main/docs/api/batch.md) and [Custom Transfer Agent](https://github.com/git-lfs/git-lfs/blob/main/docs/custom-transfers.md) documentation.
