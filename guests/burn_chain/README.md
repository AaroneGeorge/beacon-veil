# burn_chain on RISC Zero 3.1.0 (separate workspace)

Guest `burn_chain` proves the statement of `crates/lez_chain/src/statement.rs`: a burn of this note (recomputed from
the burn secret, recipient and fee) is transaction `burn_tx` of LEZ block `burn_block_id`, the block does not list
it as reverted, and blocks `pinned_id + 1 ..= tip` are one hash chain from the pinned `(id, hash)`. The journal is
version 2, 15 ABI words (480 bytes): `Vault.BurnJournal` with the accumulator root replaced by `pinnedId, pinnedHash,
tipId, tipHash, burnBlockId`.

What a proof of it shows and what it does not:
- Proven: the burn transaction for this note is transaction i of the block with hash H_N; the block does not list it
  as reverted; its bytes show the bridge program delivering `Append{instance, token, denom, note}` to the instance's
  accumulator (private burn), or a holder's `Burn` root call to the bridge with a config whose instance is this one
  (public burn); and blocks P..N..T are one unbroken hash chain with rising ids from the pinned (P, hash).
- Attested: that the hash of block T is on the real, final LEZ chain (one input; today our key, after a delay).
- Not proven, still trusted: that LEZ executed those blocks correctly (rc3 headers carry no state root and public
  execution is unproven, so "not reverted" and "the bridge program sent it" rest on the sequencer having run and
  verified the transaction), finality, and which chain is the real one (anyone can build a hash chain from the
  pinned block; the pinned block names the chain, it does not stop a dishonest attester).
- The public-burn route is tested only on a synthetic block (`fixtures/lez-chain/synthetic/2590`): no public burn
  exists on any chain we run.

Layout as `guests/burn_proof/r31`: `methods/guest` (the guest), `methods` (risc0-build 3.0.7, embeds the ELF and image
id), `host` (`burn_chain_host image-id | execute ...`, executor only).

```
export RISC0_HOME=$B/run/w5/risc0-31 CARGO_TARGET_DIR=<own target dir>
export RISC0_SERVER_PATH=$RISC0_HOME/extensions/v3.1.0-cargo-risczero-x86_64-unknown-linux-gnu/r0vm
cargo build --release --manifest-path guests/burn_chain/Cargo.toml -p burn_chain_host
burn_chain_host execute --witness-json W --dir fixtures/lez-chain/synthetic --pinned-dir fixtures/lez-chain/seq-dev \
  --pinned 2585 --tip 2591 --burn-block 2586 --tx 0 --out OUT
```
The image id here comes from the local rzup toolchain; a reproducible (docker) build and its id come with the proof.
