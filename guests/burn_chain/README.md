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

The vault side is `contracts/src/VaultChain.sol`: `Vault` plus `releaseChain`. Its chain route keeps its own release
window against the token's cap, so a vault that also released through `release` or `releaseOpen` could pay up to twice
the cap in one window. On Sepolia this vault releases through the chain route only (no root is posted to its root
anchor, so neither `release` nor `releaseOpen` can pass), so the doubled-cap case cannot arise there.

Layout as `guests/burn_proof/r31`: `methods/guest` (the guest), `methods` (risc0-build 3.0.7, embeds the ELF and image
id), `host` (`burn_chain_host image-id | prove --mode execute|groth16 ...`; `bridge_cli chain-witness` writes its
input from a live sequencer; groth16 only in a proving window).

```
export RISC0_HOME=$B/run/w5/risc0-31 CARGO_TARGET_DIR=<own target dir>
export RISC0_SERVER_PATH=$RISC0_HOME/extensions/v3.1.0-cargo-risczero-x86_64-unknown-linux-gnu/r0vm
cargo build --release --manifest-path guests/burn_chain/Cargo.toml -p burn_chain_host
burn_chain_host prove --mode execute --witness-json W --dir fixtures/lez-chain/synthetic --pinned-dir fixtures/lez-chain/seq-dev \
  --pinned 2585 --tip 2591 --burn-block 2586 --tx 0 --out OUT
```
Image id (reproducible docker build, `BV_GUEST_DOCKER=1`, risczero/risc0-guest-builder:r0.1.97.0; two builds gave the
same ELF, `evidence/bridge/chain-anchor/guest.json`): `0x88c00a4e43fc6b8b7b776e7a540422d1c684251751940901ce3256eecbd26dd3`.
Without `BV_GUEST_DOCKER=1` the local rzup toolchain builds another ELF (id `0xaa506042...d4db46`); vaults use the
docker one.
