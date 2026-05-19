# remerge Roadmap

This file is the active task tracker for the current remerge transport and
state-convergence work.

## Goal

The main client is the single source of truth.

Running `remerge` instead of plain `emerge` must leave the client in the same
final state with respect to:

- built binary packages
- usable local binpkg cache
- portage-relevant repo state needed to reproduce the build
- distfiles and other source artifacts needed to reproduce the build
- portage metadata and related inputs required for later local `emerge` runs

The server should receive only the minimum data required from the client to
produce, cache, and serve the requested artifacts.

Audit legend for this pass: `[✓]` verified from current staged code and targeted test runs, `[ ]` demoted because the current implementation or coverage does not satisfy the claim.

## Decisions

- [✓] Final-state parity means reconciling everything `remerge` touched, not only binaries.
- [✓] The server should persist submitted workorders once blob-backed submission is the default path.
- [✓] Large blob transport should use a pull-oriented model.
- [✓] Pull-oriented transport must ride the normal client-initiated connection; the server cannot assume it can reach the client directly out of band.
- [✓] Blob return over the client-initiated connection may use either bidirectional streaming or a temporary server-issued upload URL, whichever proves faster and more stable.
- [✓] The final default for blob return over the client-initiated connection should be bidirectional chunk streaming.
- [✓] The bidirectional chunk-streaming transport should use hybrid framing: text control messages plus binary data chunks.
- [✓] The chunk-streaming transport should support resumable uploads with per-chunk acknowledgements and adaptive chunk sizing for backpressure.
- [✓] Hybrid stream v1 should use JSON control frames plus self-describing binary chunks.
- [✓] Hybrid stream v1 should start with 10 MB chunks, acknowledge each chunk, grow on healthy links, and shrink on stalls.
- [✓] Cross-client reuse should use global content-addressed deduplication.
- [✓] Global blob dedup must not imply a direct blob-browsing API for clients.
- [✓] Manifest transport can be a breaking cutover rather than a long compatibility window.
- [✓] Cleanup policy should ship with defaults plus operator-configurable retention values.
- [✓] Aggressive size-pressure eviction remains out of scope; only age-based cleanup above a configurable global minimum retained-size floor is in scope.
- [✓] Persisted workorders should store immutable blob and tree digest references, not absolute filesystem paths.
- [✓] Persisted blob and tree references should be encoded in an explicit versioned manifest object with typed digest lists.
- [✓] The versioned manifest object should be embedded directly in the persisted workorder.
- [✓] Manifest evolution should happen at the embedded manifest-object level rather than through a separate outer workorder envelope.
- [✓] Embedded manifest v1 should carry digest, size, and mtime for each referenced entry.
- [✓] Embedded manifest v1 should encode mtime as Unix timestamp seconds.
- [✓] Final-state parity must include repo metadata and eclass/cache indexes needed by later local runs.
- [✓] Final-state parity must also include manifest-related metadata and repo cache artifacts used by later local runs.
- [✓] Repo-derived parity should include everything under repo metadata directories that the worker touched.
- [✓] Final-state parity must also account for relevant portage cache and metadata timestamps.
- [✓] Final-state parity must include generated package indexes or derived metadata outside repo metadata directories when later local runs depend on them.
- [✓] Final-state parity should target a byte-for-byte Portage cache and metadata snapshot when remote builds touch artifacts that later local runs may consult.
- [✓] The current mandatory byte-for-byte parity set should include full /var/lib/portage state, per-repo metadata directories, eclass cache, and the Packages index; broader temporary build caches remain out of scope unless Phase 5 proves they are required.
- [✓] Retention policy should stay global-only for now; differentiated retention classes are not part of the first cleanup implementation.
- [✓] Retention policy should remain global-only permanently unless a future requirement proves otherwise.
- [✓] Cached snapshot data should maintain a configurable global minimum retained-size floor, defaulting to 10 GiB, below which age-based eviction does not remove additional data.

## Invariants

- [✓] The client remains the authoritative source for build inputs.
- [✓] Remote builds are non-interactive by default.
- [✓] Remote builds can reproduce local-only or no-longer-upstream package versions.
- [✓] Remote builds leave reusable outputs in the client's local environment.
- [✓] The server deduplicates transferred data across workorders and clients.
- [✓] Worker containers stay disposable and do not become the source of truth.
- [✓] A later plain local `emerge --usepkg` can reuse artifacts produced by `remerge`.
- [✓] Cached snapshot data is cleaned up with delayed eviction rather than immediate deletion.
- [✓] Unreferenced snapshot data remains reusable for a grace period after client or worker churn.
- [✓] Snapshot storage has a hard upper retention bound so it cannot grow forever.

## Phase 0: Stabilize Current Behavior

- [✓] Force worker-side `emerge` and sync flows into non-interactive mode by default.
- [✓] Preserve explicit user `--ask` overrides when intentionally requested.
- [✓] Sync completed remote binpkgs back into the client's local `PKGDIR`.
- [✓] Prefer local `file://PKGDIR` reuse for the post-build local install step.
- [✓] Document the local binpkg reuse path.

## Phase 1: Capture Client State

- [✓] Snapshot non-Gentoo local overlay working trees on the client.
- [✓] Snapshot Manifest-backed distfiles needed for local or dropped upstream versions.
- [✓] Keep test roots isolated from live host `portageq` lookups.
- [✓] Add shared types for transported repo and distfile snapshots.
- [✓] Add focused tests for client-side snapshot capture.

## Phase 2: Stage Worker Runtime From Client State

- [✓] Stop passing full workorders through a large environment variable.
- [✓] Stage per-workorder runtime directories under server state.
- [✓] Mount staged workorder files into worker containers.
- [✓] Restore staged repo snapshots into worker repo locations.
- [✓] Restore staged distfiles into the worker distfiles cache.
- [✓] Add focused tests for staged runtime creation and worker restore.
- [✓] Validate the Docker `start_worker` path against the new staged-runtime contract.

## Phase 3: Server-Side Deduplicated Snapshot Storage

- [✓] Add a content-addressed blob store under the server state directory.
- [✓] Materialize staged runtime files from stored blobs instead of unique copies.
- [✓] Deduplicate identical snapshot payloads across workorders.
- [✓] Add blob-store tests proving content-hash reuse.
- [✓] Add repo tree manifest storage for staged snapshot metadata.
- [✓] Record blob references and repo tree digests in staged workorder metadata.
- [✓] Keep worker behavior unchanged while the storage model evolves underneath.

## Phase 4: Replace Inline Snapshot Transport With Manifest Negotiation

- [✓] Define the submission contract for manifest-first snapshot transport.
  - [✓] Decide which snapshot fields remain inline versus reference-only.
    - Breaking target state: repo and distfile payloads stop travelling inline in normal workorder submission.
    - Portage config keeps reference metadata inline; large payload transfer moves to negotiated blob transport.
  - [✓] Define request and response shapes for missing-blob negotiation.
  - [✓] Define failure behavior for incomplete uploads or mismatched digests.
- [✓] Add server API endpoints for snapshot negotiation.
  - [✓] Query which blob digests are already present on the server.
  - [✓] Upload missing blobs.
  - [✓] Attach uploaded blob references to a submitted workorder.
- [✓] Teach the CLI to submit manifests and upload only missing blobs.
  - [✓] Compute blob digests client-side.
  - [✓] Build repo tree manifests client-side.
  - [✓] Retry failed uploads safely.
- [✓] Add tests for end-to-end manifest negotiation and missing-blob upload.
  - [✓] Cover bidirectional chunk-streaming blob transport over the normal client-initiated connection.
  - [✓] Cover hybrid framing: text control messages plus binary data chunks.
  - [✓] Cover resumable uploads with per-chunk acknowledgements and adaptive chunk sizing.
  - [✓] Cover JSON control frames plus self-describing binary chunk headers.
  - [✓] Cover the adaptive 10 MB default chunk policy, including growth and shrink behavior.
  - [✓] Cover the breaking cutover from inline payload submission to negotiated refs.
- [✓] Treat hybrid stream v1 as fixed by this wire contract:
  - [✓] JSON control frames use UTF-8 objects with a required `type` field and a protocol `version` field set to `1`.
  - [✓] Required control message types are:
    - `upload_init`: announces `workorder_id`, `digest`, `total_size_bytes`, `chunk_size_bytes`, and optional capability flags.
    - `upload_resume`: tells the client the next byte offset and next chunk sequence to send after reconnect or restart.
    - `upload_ack`: acknowledges exactly one completed chunk with `sequence`, `offset_bytes`, `size_bytes`, and cumulative `received_bytes`.
    - `upload_complete`: confirms the full blob was accepted and validated.
    - `upload_error`: reports a terminal protocol or validation failure with machine-readable `code` and human-readable `message`.
  - [✓] JSON control field rules:
    - [✓] `version` is an integer and must equal `1` for v1 messages.
    - [✓] `digest` is lowercase SHA256 hex without alternate prefixes.
    - [✓] `workorder_id` uses the normal UUID string form.
    - [✓] `offset_bytes`, `size_bytes`, `total_size_bytes`, `received_bytes`, and `sequence` are unsigned integers.
  - [✓] Self-describing binary chunk header for each chunk is fixed to:
    - 4 bytes magic: `RMCH`
    - 1 byte protocol version: `1`
    - 1 byte flags: reserved in v1, set to `0`
    - 2 bytes reserved: set to `0`
    - 8 bytes chunk sequence, big-endian
    - 8 bytes chunk offset bytes, big-endian
    - 8 bytes payload size bytes, big-endian
    - 4 bytes chunk checksum, Adler-32 of payload, big-endian
    - followed immediately by the raw payload bytes for that chunk
  - [✓] Chunk policy rules:
    - [✓] Start at 10 MiB chunk payloads.
    - [✓] Acknowledge each successfully persisted chunk individually.
    - [✓] Grow chunk size only after a stable run of healthy acknowledgements.
    - [✓] Shrink chunk size on stalls, slow acknowledgements, or reconnect-driven resume.
    - [✓] Never exceed negotiated blob length or send overlapping byte ranges except deliberate replay from the resume offset.
  - [✓] Resume and backpressure rules:
    - [✓] Server authority decides the next accepted offset; clients resume exactly from `upload_resume`.
    - [✓] Clients may have at most one unacknowledged chunk in flight in v1.
    - [✓] A missing acknowledgement is treated as no progress; clients must not advance local send offset speculatively.
    - [✓] Duplicate replay of the last acknowledged chunk is allowed only when reconnect semantics require replay from the confirmed resume point.

## Phase 4.5: CLI Sync Feedback

- [✓] Show live sync progress while the CLI populates the local binpkg cache.
  - [✓] Add a progress bar covering current package, bytes synced, throughput, and ETA.
  - [✓] Surface cache hits clearly so repeated syncs visibly demonstrate reuse.
  - [✓] Show a final sync summary with reused-versus-downloaded packages and bytes.
  - [✓] Thread progress reporting through the download path without changing sync correctness.
  - [✓] Add tests for progress reporting and cache-hit visibility during repeated syncs.
  - [✓] Follow this UX contract so implementation does not reopen design questions:
    - [✓] State 1, sync start: print the binhost URI and total number of built packages to inspect.
    - [✓] State 2, package inspection: classify each package as `[CACHE-HIT]` or `[DOWNLOAD]` before any transfer begins.
    - [✓] State 3, active download: show package atom/path label, bytes transferred, total bytes, throughput, and ETA for the current package.
    - [✓] State 4, index refresh: show a distinct refresh step for the `Packages` index after package blobs are handled.
    - [✓] State 5, final summary: show downloaded count and bytes, reused count and bytes, total elapsed sync time, and the PKGDIR location.
  - [✓] Use this cache-hit rule:
    - [✓] Treat a package as a cache hit only when the destination file already exists, matches the expected size, and matches the expected SHA256.
    - [✓] Skip transfer entirely for verified cache hits and count them in the reuse summary.
  - [✓] Use these progress update rules:
    - [✓] Update visible byte progress at a human-stable cadence rather than per-chunk spam.
    - [✓] Show ETA only for active downloads, never for cache hits.
    - [✓] Replace ETA with a stalled indicator if throughput collapses long enough that the ETA becomes misleading.
  - [✓] Keep failure semantics simple:
    - [✓] Preserve the existing atomic `.part` download behavior.
    - [✓] If a package download fails, stop sync, report the failing package, and print partial summary information for already reused/downloaded packages.

## Phase 4.6: Blob Compression

- [✓] Add compression-aware blob transport and storage without changing digest identity.
  - [✓] Add blob metadata support for canonical raw digests plus optional encoded variants.
  - [✓] Preserve raw-byte digest identity across storage, staging, parity, and transport.
  - [✓] Negotiate blob upload compression over the existing blob stream control channel.
  - [✓] Support compressed blob downloads using standard HTTP content encoding.
  - [✓] Extend blob and tree metadata only where needed to describe stored encoded variants; parity transport relies on HTTP content encoding rather than a separate manifest field.
  - [✓] Apply entropy-gated zstd compression for blobs when worthwhile.
  - [✓] Apply the same compression policy to stored tree manifests.
  - [✓] Add focused blob-store and API tests for compressed transport and raw-digest verification.
  - [✓] Add focused parity tests for compressed transport and raw-digest verification.
  - [✓] Document the compression model and operational behavior.

## Phase 5: Client Final-State Convergence

- [✓] Define what "same final state as emerge" means in testable terms.
  - [✓] Binaries
  - [✓] local cache contents
  - [✓] distfiles
  - [✓] repo inputs
  - [✓] relevant portage metadata inputs
  - Decision: parity covers everything `remerge` touched that affects later local `emerge` behavior.
- [✓] Audit which client-side state still diverges after a remote build.
  - [✓] repo metadata
    - Include eclass/cache indexes and other derived repo metadata needed by later local runs.
    - Include manifest-related metadata and repo cache artifacts used by later local runs.
    - Include everything under repo metadata directories touched by the worker.
    - Current implemented slice: regular files under `/var/db/repos/*/metadata/**` are captured on the worker, stored in the blob store, restored on the client, and verified by digest plus mtime.
  - [✓] fetched source artifacts
    - Current implemented slice: the worker emits a digest/size/mtime manifest for final-state distfiles, the server ingests those blobs into the shared blob store, and the client restores missing or stale distfiles into the local `DISTDIR` before any follow-up local `emerge`.
  - [✓] package metadata or indexes needed by later local runs
    - Include generated package indexes or derived metadata outside repo metadata directories when later local runs depend on them.
    - Current implemented slice: capture and restore `/var/cache/binpkgs/Packages` plus repo-level `/var/db/repos/*/Packages` when present.
  - [✓] portage cache and metadata timestamps needed for honest parity
    - Current implemented slice: regular files, directories, and symlinks under the approved parity roots are captured and restored with preserved mtime-to-the-second.
  - [✓] full byte-for-byte Portage cache and metadata snapshot scope
    - Mandatory parity set: full /var/lib/portage state, per-repo metadata directories, eclass cache, and Packages index.
    - Snapshot and restore these path sets byte-for-byte, preserving file content and mtime to the second:
      - `/var/lib/portage/**` for regular files, directories, and symlinks.
      - `/var/db/repos/*/metadata/**` recursively for every active repo.
      - `/var/cache/eclass/**` recursively for the configured eclass cache.
      - `/var/cache/binpkgs/Packages` for the local PKGDIR index.
      - `/var/db/repos/*/Packages` when a repo-level Packages index exists.
    - Exclude these paths from the parity snapshot because they are synced elsewhere, client-owned, or regenerable temporary state:
      - `/var/cache/binpkgs/*.gpkg.tar*` and other binpkg payload files.
      - `/var/cache/portage/**` temporary build cache.
      - `/var/cache/distfiles/**` source payloads already handled by distfile snapshot transport.
      - `/var/db/pkg/**` installed-package VDB owned by the client.
      - Version-control directories such as `.git/` and `.hg/` under repo trees.
    - Treat sockets, FIFOs, lockfiles, and transient temp files under these trees as non-parity runtime noise unless later Phase 5 evidence proves otherwise.
    - Implement parity capture using path-set rules rather than case-by-case heuristics so reconciliation is deterministic and testable.
  - [✓] any transient worker-only state that should instead live on the client.
- [✓] Implement the missing client reconciliation steps.
  - [✓] Capture
    - [✓] Collect the parity include set from the worker after the remote build reaches its final state.
    - [✓] Record each captured parity entry as content plus preserved mtime-to-the-second metadata.
    - [✓] Exclude configured non-parity paths and runtime noise during capture rather than filtering later.
    - [✓] Emit a deterministic parity manifest for captured paths so later transfer and verification use the same file inventory.
    - [✓] Initial approved-path slice: collect regular files under `/var/lib/portage/**`, `/var/db/repos/*/metadata/**`, `/var/cache/eclass/**`, `/var/cache/binpkgs/Packages`, and repo-level `/var/db/repos/*/Packages` after the worker reaches final build state.
    - [✓] Initial approved-path slice: emit a deterministic manifest with SHA256 digest, size, and mtime-to-the-second for each captured file.
    - [✓] Initial approved-path slice: exclude non-parity paths and runtime noise by traversing only the approved path set and recording regular files only.
  - [✓] Transfer
    - [✓] Reuse the same manifest/blob transport rules as other snapshot payloads wherever possible.
    - [✓] Transfer only missing parity blobs to the client side when the local copy is absent or mismatched.
    - [✓] Keep parity transfer logically separate from binpkg sync so failures are attributable to parity reconciliation rather than package download.
    - [✓] Initial approved-path slice: reuse the shared blob store plus digest-addressed transfer path for parity payloads.
    - [✓] Initial approved-path slice: download only mismatched approved-path parity blobs and keep parity reconciliation as a distinct post-sync step.
  - [✓] Restore
    - [✓] Materialize restored parity files only inside the approved include set.
    - [✓] Restore bytes first, then restore preserved mtime to the second.
    - [✓] Apply directory creation and file replacement atomically enough to avoid partially reconciled visible state after interruption.
    - [✓] Avoid mutating excluded client-owned paths such as VDB, distfiles, temporary build caches, and binpkg payload files.
    - [✓] Initial approved-path slice: restore only under the approved parity roots, write via `.part` files, then atomically rename into place.
  - [✓] Verification
    - [✓] Verify restored paths against the parity manifest using digest and mtime checks.
    - [✓] Detect and report any skipped, excluded, or mismatched parity paths explicitly.
    - [✓] Initial approved-path slice: verify restored files with digest, size, and mtime checks before accepting parity success.
    - [✓] Ensure remote build side effects needed later are reflected locally before the CLI proceeds to any follow-up local emerge step.
    - [✓] Fail closed when mandatory parity paths cannot be reconciled, unless a future explicit degraded-mode policy is introduced.
- [✓] Add integration tests that compare `remerge` and local `emerge` final-state outcomes.
  - [✓] Cover capture of the approved parity include set.
  - [✓] Cover parity transfer when the client is already partially up to date.
  - [✓] Cover restore and verification of bytes plus preserved mtime.
  - [✓] Cover exclusion of VDB, distfiles, temporary build caches, and binpkg payload files from parity reconciliation.

## Phase 6: Smarter Assembly and Lifecycle Management

- [✓] Add blob reference tracking and garbage collection.
  - [✓] Track which staged workorders reference which blobs and trees.
  - [✓] Record `last_referenced_at` so cleanup is based on delayed expiry rather than immediate unreachability.
  - [✓] Account for blob and tree sizes so retention decisions can use actual on-disk usage.
  - [✓] Clean up unreferenced data safely.
  - [✓] Avoid deleting data still needed by active workorders.
  - [✓] Keep newly unreferenced data for a short grace period so worker or client churn can still reuse it.
- [✓] Add smarter runtime assembly optimizations.
  - [✓] Prefer reflinks where supported.
  - [✓] Fall back to hardlinks.
  - [✓] Fall back to copies when linking is not possible.
- [✓] Add quotas, accounting, or eviction rules for stored snapshots.
  - [✓] Define a default warm-cache grace period for unreferenced client snapshot data.
  - [✓] Define a separate hard-delete retention for old unreferenced blobs and trees.
  - [✓] Define a configurable global minimum retained-size floor for cached snapshot data.
    - [✓] Default to 10 GiB retained on the server.
    - [✓] Only evict age-eligible unreferenced data when retained data exceeds the minimum floor.
    - [✓] Evict oldest eligible data first until the retained-size floor is reached again.
    - [✓] Keep the retained-size floor in force even for very old unreferenced data once the floor has been met.
  - [✓] Expose configuration for operators to tune cleanup delays.
    - [✓] Decision: ship defaults with operator-configurable overrides.
  - [✓] Expose configuration for operators to tune the minimum retained-size floor.
  - [✓] Keep retention policy global-only in the first implementation.
- [✓] Add delayed cleanup policy and implementation.
  - [✓] Baseline policy: keep unreferenced snapshot data reusable for 7 days.
  - [✓] Baseline policy: hard-delete unreferenced blobs and trees after 30 days.
  - [✓] Baseline policy: retain at least 10 GiB of cached snapshot data globally before age-based eviction removes additional data.
  - [✓] Ensure a client config change that creates a new worker does not immediately discard still-useful cached data from the old config.
  - [✓] Run cleanup asynchronously so builds are never blocked on large deletions.
  - [✓] Make cleanup idempotent and restart-safe.
- [✓] Defer aggressive size-pressure eviction until after floor-aware delayed age-based cleanup is stable.
- [✓] Document operational behavior for deduplicated storage.
  - [✓] Document the default 7-day reuse window and 30-day hard-delete window.
  - [✓] Document the default 10 GiB minimum retained-size floor and how it interacts with age-based eviction.
  - [✓] Document how operators can force cleanup or extend retention.

## Phase 7: Validation Matrix

- [✓] Unit tests
  - [✓] Blob-store dedup behavior
  - [✓] Tree manifest staging metadata
  - [✓] Worker restore from staged runtime
  - [✓] Manifest negotiation edge cases
  - [✓] Reference-tracking and GC behavior
- [✓] Integration tests
  - [✓] Docker worker startup with staged workorder path
  - [✓] CLI submit with missing-blob upload
  - [✓] Rebuild of local-only overlay package version
  - [✓] Rebuild of upstream-dropped distfile version
  - [✓] Client final-state parity versus local `emerge`
- [✓] Operational validation
  - [✓] Restart safety for staged runtimes and stored blobs
  - [✓] Large snapshot upload behavior
  - [✓] Multi-client dedup effectiveness
  - [✓] Cleanup behavior under load
  - [✓] Grace-period reuse after client config changes or worker replacement
  - [✓] Hard-delete behavior once data ages past the retention bound.

## Open Questions

No blocking roadmap questions remain for the current transport and parity design slice.

Resolved implementation defaults:

- [✓] Hybrid stream v1 uses JSON control frames plus self-describing binary chunks over `/api/v1/snapshots/blobs/stream`.
- [✓] Hybrid stream v1 starts with 10 MB chunks, acknowledges each chunk, grows on healthy links, and shrinks on stalls.
- [✓] Persisted repo/distfile snapshot refs now use the embedded manifest object to carry digest, size, and mtime; parity manifests already carry digest, size, and mtime with mtime encoded as Unix timestamp seconds.
- [✓] Current mandatory parity scope is full /var/lib/portage state, per-repo metadata directories, eclass cache, and the Packages index.
- [✓] CLI sync shows live progress, ETA, and cache-hit visibility during local binpkg cache population.
- [✓] Retention should use a configurable global minimum retained-size floor, defaulting to 10 GiB, before age-based eviction removes additional cached snapshot data.
- [✓] Exact parity include/exclude rules are defined by path-set: include `/var/lib/portage/**`, `/var/db/repos/*/metadata/**`, `/var/cache/eclass/**`, `/var/cache/binpkgs/Packages`, and repo-level `Packages` indexes; exclude binpkg payloads, distfiles, `/var/cache/portage/**`, `/var/db/pkg/**`, and VCS metadata.
- [✓] Sync progress UX uses the five-state flow: start, cache-hit/download classification, active per-package progress, Packages index refresh, and final reused-versus-downloaded summary.
- [✓] Hybrid stream v1 wire contract is fixed: JSON control frames with `upload_init`, `upload_resume`, `upload_ack`, `upload_complete`, and `upload_error`, plus `RMCH` self-describing binary chunk headers.
- [✓] Phase 5 parity reconciliation is grouped into capture, transfer, restore, and verification so implementation can proceed without reopening scope questions.

## Immediate Next Slice

- [✓] Add API support for missing-blob discovery and blob upload.
- [✓] Add CLI-side digest and tree-manifest generation for submission.
- [✓] Replace inline repo and distfile snapshot payload submission with negotiated refs.
- [✓] Add one integration test that proves the server only requests missing blobs.
- [✓] Specify the CLI sync progress model, including ETA, cache-hit reporting, and repeated-sync summaries.
- [✓] Design and implement the pull-oriented transfer replacement for the temporary upload endpoint over the normal client-initiated connection.
- [✓] Design and implement the hybrid text-control/binary-chunk protocol for snapshot blob upload.
- [✓] Define and implement per-chunk acknowledgement, resume, and backpressure rules for that protocol.
- [✓] Define and implement the JSON control schema and self-describing binary chunk header for hybrid stream v1.
- [✓] Define embedded manifest v1 field encoding for digest, size, and mtime.
- [✓] Enumerate the exact parity file and directory paths under /var/lib/portage, repo metadata, eclass cache, and Packages handling.
- [✓] Define the server retention accounting model for the 10 GiB minimum retained-size floor and floor-aware eviction order.
- [✓] Persist blob-backed workorders on the server using immutable blob and tree digest references so the breaking manifest cutover has restart safety.