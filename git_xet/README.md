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

The agent requests LFS object metadata, extracts the Xet hash from the bridge URL, obtains a scoped read token, and calls the existing Rust reconstruction engine. Objects of 64 MB or larger use the download URL directly with up to 8 parallel ranged requests (~250 MB/s versus ~65 MB/s for a single CAS stream), and unsupported servers fall back to the Xet path. Plain LFS objects retain their ordinary HTTP download path. All paths check the expected size and SHA-256; failed downloads are removed. Public downloads do not prompt for credentials. Private requests use the existing Git/HF credential machinery after an authentication challenge. The read-token route currently uses the Hub's `main` revision, including for LFS objects with no associated Hub file commit.

`git xet --verbose --log /path/to/xet.log transfer --lfs-url URL` enables diagnostics; stdout remains reserved for the Git LFS protocol.

Build from source with Rust 1.95:

```sh
cargo build --locked --release -p git_xet
cargo test --locked -p git_xet --features simulation --lib
cd git_xet
uv build --wheel
```

OpenSSL is vendored into the executable by default. Release packages strip debug symbols. Linux release archives require glibc 2.35 or newer; comma's dependency builds compile from the pinned source for their manylinux baseline. macOS and Windows fork binaries are unsigned.

For the upstream Homebrew, winget, and MSI packages, see the [upstream installation instructions](https://github.com/huggingface/xet-core/blob/main/git_xet/README.md). Those packages do not contain this fork's download implementation.
