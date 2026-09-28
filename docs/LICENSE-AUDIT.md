# License audit: Cargo.lock (2026-09-18)

Every package in `Cargo.lock` at its locked version, checked against the
crates.io registry API (one request per crate-version pair, 395 published
packages + this workspace). `cargo-deny` would automate this; it is not part
of the local toolchain, so the review ran manually. Re-run after any
dependency change.

## Result: no copyleft conflict with the MIT license

| License | Packages | Notes |
|---|---|---|
| MIT | 283 | |
| Apache-2.0 | 51 | |
| MIT OR Apache-2.0 (or `MIT/Apache-2.0` spelling) | 19 + 1 + 1 | consumer picks either |
| Unicode-3.0 | 18 | unicode data crates (permissive) |
| ISC | 5 | incl. `aws-lc-sys` pieces |
| Unlicense (± MIT) | 6 | |
| Zlib | 3 | |
| CDLA-Permissive-2.0 | 2 | `webpki-roots`, `webpki-root-certs`, root-certificate data, permissive |
| BSD-2-Clause | 2 | |
| MPL-2.0 | 1 | `option-ext`, file-level copyleft; MPL explicitly allows linking from any code, MIT-compatible |
| 0BSD | 1 | |
| CC0-1.0 | 1 | |
| MIT OR Apache-2.0 OR LGPL-2.1-or-later | 2 | `r-efi` 5.3.0/6.0.0, OR-tri-license; LGPL is one of three choices, never mandatory |

Zero GPL/AGPL/EUPL/CC-BY-NC packages. `aws-lc-sys` bundles AWS-LC
(ISC + (Apache-2.0 OR ISC) + BSD-3-Clause + MIT family), all permissive.

## MIT vs Apache-2.0 for this repository

Recorded analysis, not a decision: the MIT default stays; a switch to
Apache-2.0 would be deliberate and is not planned.

- Every locked dependency is MIT-compatible as-is. Nothing forces a change.
- The one argument for Apache-2.0 is its explicit patent grant; MIT grants
  patents only implicitly. The ONNX/runtime stack is the relevant place:
  `fastembed`, `ort`, `hf-hub` are Apache-2.0 OR MIT (so their Apache terms
  are already available to us either way), and `onnxruntime.dll` is MIT.
  Crypto: `chacha20poly1305`/`hkdf`/`hmac`/`sha2`/`subtle`/`aws-lc-rs`, Apache-2.0 OR MIT or ISC. Nothing here is single-sourced under a
  patent-encumbered license.
- Practical read: switching the repo to Apache-2.0 would buy a patent grant
  the dependency graph does not require, at the cost of a longer license
  text. If an explicit grant (or a NOTICE file policy) is wanted,
  the switch is a one-line workspace change plus this file's update.

## Verification

```sh
awk '/^name = /{n=$3} /^version = /{v=$3; gsub(/"/,"",n); gsub(/"/,"",v);
     if (n!="") print n" "v}' Cargo.lock
# then, per line: curl -s -A "<UA>" \
#   https://crates.io/api/v1/crates/$n/$v  ->  .version.license
```
