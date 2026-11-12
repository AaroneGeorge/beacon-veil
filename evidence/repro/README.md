Reproducibility evidence
- IMAGE_IDS, BUILD_INFO, SHA256SUMS: release set built 2026-11-12 from commit bf95e35 (pre-rewrite id; see the commit map) in docker risczero/risc0-guest-builder:r0.1.91.1@sha256:fafb377a...
- second-build-ci-sim.log: an independent rebuild from a fresh clone of a later commit (329bb3f, pre-rewrite id). check_image_ids.sh reported OK for all 6 guests.
- CI re-checks the ids on every push (evidence/g2/ci-run.txt, guests/IMAGE_IDS.expected).
