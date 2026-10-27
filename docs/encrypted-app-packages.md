# Encrypted BC `.app` packages — identified, and read

Status: **identified 2026-10-27.** These are BC **runtime packages** (the `.NEA` form),
scrambled with a fixed, **public** RC4 key — obfuscation, not secrecy. The format is
community-documented (hougaard.com's runtime-app writeups, `SimonOfHH/D365BCAppHelper`,
`businessdev.ALbuild`'s `Expand-BcAppFile`); nothing was brute-forced. al-sem descrambles
and reads them like any other package (see "What al-sem does"). The earlier state of this
file ("no decryption attempted", "scheme and key unknown", "trailer not identified") is
superseded by the sections below.

## Samples

`/home/sshadows/Projects/CONtinAI/work/ws/{DO,CDN,CDN-Onboarding,DO-onprem}/.alpackages/*.app`

- 36 files, all in the runtime form.
- Only **10 distinct packages**: the 36 files are byte-identical copies (same SHA-256)
  spread over the four workspaces. Each workspace holds the other apps, not itself.
- Packages: CDN, CDN Onboarding, DO, DO on-premises, Core, Continia Business
  Foundation, Continia Connector App, Continia Customer Onboarding, Continia Online
  Connector, Continia System Application. All version 29.0.0.x (BC 2026 Wave 2).

## File layout

Offsets in bytes; integers are little-endian.

| Offset | Size | Value on every sample | Meaning |
|--------|------|-----------------------|---------|
| 0 | 4 | `NAVX` | magic |
| 4 | 4 | 40 | header size |
| 8 | 4 | 2 | header version |
| 12 | 16 | varies | a GUID (.NET `bytes_le` order) |
| 28 | 8 | varies | **RC4 stream length** (from offset 48, not 40) |
| 36 | 4 | `NAVX` | magic again |
| 40 | 8 | `2E 4E 45 41 00 00 00 01` (`.NEA`, then `00 00 00 01`) | the runtime-package mark |
| 48 | stream length | high-entropy bytes | **the RC4 stream** — descrambles to a plain zip |
| 48 + stream | 14,024–14,045 B | `NXSB` + DER | a PKCS#7/CMS `signedData` signature (see "The trailer") |

The 40-byte header is the same NAVX header a normal package has. In a normal package
the payload at offset 40 is a zip (`PK\x03\x04`); here it is the `.NEA` mark.

**The length field at offset 28 counts from offset 48** — the 8-byte mark is NOT part
of it. Cutting at `40 + length` breaks the zip's end-of-central-directory; `48 + length`
lands it exactly on the last byte. (This detail cost one failed probe before it was
pinned; the community samples show the same layout.)

Correction to the first report: the trailer is file size − 40 − payload length,
14,024–14,045 bytes. The earlier figure (~14,065–14,085) left out the 40-byte header.
(The trailer lengths above are as measured on the distinct packages, 14,031–14,045.)

## The scramble

RC4 with the fixed 6-byte key `15, 11, 81, 137, 184, 120`, applied from offset 48.
The key is public and has been community-documented for years; `.NEA` is the mark BC
has shipped on runtime packages all along ("Net Encrypted Archive" was Erik Hougaard's
guess at the name). Nothing here is Continia-specific or new in BC 2026 Wave 2: all 36
sample files descramble to zips whose every entry CRC passes, with the same key.

Runtime packages are produced by asking a BC server for the runtime form of an
installed app (`Get-BcContainerAppRuntimePackage` in BcContainerHelper), not by the AL
compiler; inside, AL source is typically replaced by transpiled `bin/*.cs` — except
that **9 of these 10 carry their AL `src/` on purpose** (Continia's
`resourceExposurePolicy.allowDownloadingSource: true`), so the source is readable too.

## The trailer

`NXSB` (4 bytes) followed by a DER-encoded PKCS#7/CMS `signedData` (OID
1.2.840.113549.1.7.2, SHA-256 digest) — a code-signing signature. The signer certificate
is `Continia Software A/S`, chained through `Microsoft ID Verified CS EOC CA 04` →
`Microsoft ID Verified Code Signing PCA 2021` → `Microsoft Identity Verification Root CA
2020`: an Authenticode-style chain, i.e. Continia signed these packages with their
code-signing cert and Microsoft's "ID verified" CA vouches for the identity. Nothing reads
or verifies it (the zip ends before it); it is not needed to read the package.

## The header GUID

- It is **not** the app id. CDN's header says `e211b728-c35b-4e68-a4b2-2162cebfb32b`;
  CDN's real id (from the descrambled manifest, matching its `app.json`) is
  `0745e76d-0b72-4641-87c2-ee45db5d2c32`.
- It is stable per package build: every copy of the same package carries the same GUID.
- Its role (package id, build id, key id) remains unknown.

Per-package values:

| Package | Header GUID | Payload length | Trailer |
|---------|-------------|----------------|---------|
| CDN 29.0.0.213759 | e211b728-c35b-4e68-a4b2-2162cebfb32b | 7,548,619 | 14,045 |
| CDN Onboarding 29.0.0.213759 | 31dd5c2e-0025-4efb-aa7f-ed1603d4cb76 | 548,902 | 14,044 |
| DO 29.0.0.213763 | 0f34353a-05d7-48e0-92a7-79190398bfa6 | 14,015,741 | 14,045 |
| DO on-premises 29.0.0.213763 | 48100d86-c986-472f-bc52-67124eac8b51 | 113,765 | 14,044 |
| Core 29.0.0.221304 | 5e938447-ec8d-44ad-a3e1-e3776180215e | 4,126,901 | 14,031 |
| Continia Business Foundation 29.0.0.222757 | 8ec6b6d8-b4ee-4481-b297-e065aa31e18d | 347,796 | 14,025 |
| Continia Connector App 29.0.0.222757 | 29f4a894-489a-48d1-8cbf-feb49201d76e | 75,942 | 14,025 |
| Continia Customer Onboarding 29.0.0.219828 | 9291cdee-a7cd-4aa7-8ec8-55458ec3975d | 556,641 | 14,024 |
| Continia Online Connector 29.0.0.222757 | 9fb04fd3-baa7-48c3-9798-2ddb3917dd86 | 668,864 | 14,025 |
| Continia System Application 29.0.0.222757 | 76fcb020-7f3d-4d81-bd29-b47f53398687 | 519,232 | 14,025 |

## What al-sem does

`src/engine/deps/app_package_zip.rs` (branch `continai-gaps`) detects the `.NEA`
mark, descrambles the stream with the public RC4 key, and opens the plain zip:
manifest, symbols and embedded source then read through the same public readers
as any plain package (`open_app_file`, `app_zip_bytes`), so the dependency loader,
ABI ingest and the LSP's embedded-source path get the real app.

`EncryptedApp` remains as the **failure channel**: a `.NEA`-marked package whose
stream cannot be descrambled (truncated file, stream past the memory cap, not a
zip) is reported by name (`payload.summary.encryptedPackages`, G3) and counted as
an unreadable dependency for that stated reason — a future format or key change
from the publisher surfaces loudly, not as silent absence.

## Not known

- What the `00 00 00 01` after `.NEA` means (the community samples show the same
  bytes; a format version is a guess).
- The header GUID's role (package id? build id?) — stable per package build, but
  not the app id.
- Whether BC verifies the `NXSB`/CMS trailer at install time.

## Getting the content

The scramble is public, so none of the old workarounds are needed any more — but
they remain the fallback if the format ever moves:

1. **Descramble and read it** (what al-sem does now): manifest, symbols and source.
2. **Use the source we have.** DO, CDN, CDN-Onboarding and DO-onprem have their
   source in `ws/*/` as well; match a package to a workspace by `app.json`
   id/name/publisher/version (not by the header GUID). This covers 4 of the 10.
3. **Symbol packages from a BC server.** The dev endpoint (`/dev/packages`) returns
   symbol packages for published apps, normally with `SymbolReference.json` readable.
   The BC 28.4 sidecar (`bc-dev`) could test whether an app published from a runtime
   package comes back this way.
4. **Ask the publisher** for plain or symbols-only builds.
5. **Report it clearly.** When none applies (a future key or format change), al-sem
   names the unreadable dependency and counts edges into it as unresolved for that
   stated reason.
