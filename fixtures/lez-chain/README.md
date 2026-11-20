# LEZ v0.4.0-rc3 block fixtures (crates/lez_chain)

Served block bytes (`borsh(Block)`), one file per block, `<id>.borsh`. Details and hashes: `manifest.json`.
Written by `tools/lez_chain_probe/src/bin/synth.rs` with LEZ's own types.

- `seq-dev/2581..2591`: **real** blocks of the dev-mode sequencer seq-dev (2026-11-12), copied byte for byte. Burn 0
  (block 2586) and burn 1 (block 2588) are private burns: a privacy-preserving transaction whose boundary step 0
  delivers `Append` from the bridge program to the instance's burn accumulator. Their burn secrets are not here.
- `synthetic/2586..2591`: a **synthetic** chain forked after real block 2585. Block 2586 is real burn 0 with its
  `Append` note and memo replaced by ones from a test secret (its proof no longer matches); block 2590 holds one added
  public `Burn` transaction signed by a test key, the only public burn in the fixtures (no public burn exists on any
  chain we run). No node would accept these blocks. The witnesses (test secrets) are in `manifest.json`.

Tests: `cargo test --manifest-path crates/lez_chain/Cargo.toml`.
