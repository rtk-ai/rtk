# Verifying Release Binaries

All RTK release artifacts include SHA-256 checksums for integrity verification.

## Manual Verification

1. Download the binary archive and its .sha256 file from the [releases page](https://github.com/rtk-ai/rtk/releases)

2. Verify:
   ```bash
   sha256sum -c rtk-x86_64-unknown-linux-musl.tar.gz.sha256
   ```

3. Or check against SHA256SUMS.txt:
   ```bash
   sha256sum -c SHA256SUMS.txt
   ```

## Verify After Installation

After installing via install.sh, the checksum is automatically verified (since v0.36.0).

## For Maintainers: Setting Up Cosign Signing

1. Generate a cosign keypair:
   ```bash
   cosign generate-key-pair
   ```

2. Add the private key as a GitHub secret: COSIGN_KEY

3. Add to release workflow after checksum generation:
   ```yaml
   - name: Sign checksums
     uses: sigstore/cosign-installer@v3
     with:
       cosign-release: v2.2.0
   - run: echo "$COSIGN_KEY" | cosign sign-blob --key - dist/SHA256SUMS.txt > dist/SHA256SUMS.txt.sig
   ```
