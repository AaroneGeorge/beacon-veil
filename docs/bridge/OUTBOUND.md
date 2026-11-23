# Outbound: LEZ burn to Ethereum release

Status: spike. Code: `programs/bridge` (Burn, accumulator), `guests/burn_proof`, `contracts/src/Vault.sol` (`release`), `contracts/src/StubLezAnchor.sol`, `tools/bridge_cli`. Chain route: `crates/lez_chain`, `guests/burn_chain`, `contracts/src/VaultChain.sol` (`releaseChain`), `contracts/src/StubLezTipAnchor.sol`, `contracts/src/CommitteeLezAnchor.sol` (3 of 4, stand-in key set).

## Summary

1. The holder burns one denomination of the wrapped token on LEZ, from a private (default) or public account (F6). The burn appends a leaf to the instance's burn accumulator. The leaf hides the Ethereum recipient (F5).
2. An anchor tells the vault that an accumulator root is final. In the sprint this is `StubLezAnchor`: the owner posts the root (on one test vault it is `CommitteeLezAnchor`: 3 of 4 stand-in keys, all ours). That is a stated trust assumption (TRUST.md).
3. Anyone runs guest `burn_proof` for the burn and gets a Groth16 seal. `Vault.release(seal, journal)` checks the proof with RISC Zero's verifier and pays the recipient (F7).

## Hashes (wire format)

| Value | Definition |
|---|---|
| burn secret j | `sha256("wb/burn/v1" \|\| key \|\| bridge_instance \|\| j_le8)` |
| note | `sha256("wb/b/v1" \|\| burn_secret \|\| recipient \|\| word(fee))` |
| accumulator leaf | `sha256("wb/bleaf/v1" \|\| token \|\| word(denom) \|\| note)`, computed by the program |
| release nullifier | `sha256("wb/bn/v1" \|\| burn_secret)` |
| memo | `(recipient \|\| fee_be16) XOR sha256("wb/memo/v1" \|\| burn_secret \|\| ctr)`, 36 bytes |

The program builds the leaf from the token and denomination it really debited. So the token and amount the proof reveals are the burned ones. The note binds the recipient and the relayer fee (Security 3). A relayer who submits the release cannot change either. The memo is published with the burn, so the owner can rebuild the release from the seed alone (F9). To anyone without the burn secret it is 36 pseudorandom bytes.

## LEZ side

- `Burn{config, token, denom, note, memo}` on the holder's actor. It needs `is_authorized` (the holder signed, or owns the private account). It debits `denom + burn_fee` from `(instance, token)` (`WB:B04` if short) and credits the fee to the treasury. It then sends `Append` to the accumulator `PDA(bridge, sha256("bridge/acc/v1" || instance))`.
- `Append` (own program only) inserts the leaf into the incremental tree (same algorithm and depth as the vault). It stores the new root, emits event `bridge::BurnAppended`, and writes the leaf, once, to the leaf record `PDA(bridge, sha256("bridge/bleaf/v1" || instance || index))`.
- The leaf records are a sprint shortcut. Our standalone sequencers run no indexer, and LP-0012 events are served only by LEZ's indexer service. Each record costs about 200 bytes of public state per burn. The milestone design reads the events through the indexer, and the records go away.
- A burn transaction declares the leaf record of the next index. If another burn lands first, this one fails whole: nothing is debited (R2), and the CLI retries.
- A burn from a private account is a hybrid transaction: the debit is proven privately, and the append runs publicly. On chain one sees the token, the amount, the time, the note and the memo. The holder and the recipient are not visible.

## Guest burn_proof

Private input: `BurnWitness{burn_secret, recipient, fee, token, denom, leaf_index, path, chain_id, vault, lez_program, lez_instance}`. The guest recomputes note, leaf, root and nullifier (`bridge_program::journal::prove_burn`, the same function the CLI runs natively). It commits a 352-byte journal of 11 ABI words, the Solidity struct `Vault.BurnJournal`: version, chainId, vault, lezProgram, lezInstance, accumulatorRoot, nullifier, recipient, token, denom, fee.

The guest proves membership in a root. It does not prove that the root is the real, final accumulator of that instance. That is the anchor's statement (see stages below).

## Vault side

`release(seal, journal)`:

1. The journal is 352 bytes and version 1.
2. The pairing (F15): `chainId == block.chainid`, `vault == this`, `lezProgram` and `lezInstance` equal to the vault's immutable program id and its once-bound instance.
3. The token is releasable and not frozen (F14 stub). The denomination is in the token's list, and `fee < denom`.
4. The nullifier is unspent (R3).
5. `anchor.isFinal(lezInstance, accumulatorRoot)`.
6. `verifier.verify(seal, burnImageId, sha256(journal))` reverts on a bad proof.
7. The window cap applies (F11 stub). Then mark the nullifier, pay `denom - fee` to the recipient, and pay `fee` to the caller (the relayer).

A failed payout (for example a recipient contract that rejects ETH) reverts the whole call. The nullifier stays unspent and the entitlement stays intact (R2; Foundry `test_failedReleaseKeepsEntitlement`).

Verifier choice (revised 2026-11-12). RISC Zero advisory GHSA-g6qg-9j8p-7j87 (risc0) and GHSA-mm6c-35cm-g4x2 (risc0-ethereum), both 2026-11-11: the Groth16 verifier up to 3.0.0 and `risc0-zkvm` below 3.1.0 accept a chosen invalid receipt claim. Vault v3 holds the RISC Zero Groth16 verifier 3.1.0 through its emergency-stop wrapper, as an immutable: Sepolia estop `0xA1DCB16cb765b3CA71605339F0C37Bfe0F60bA29` around verifier `0x8c915b5071206C6E328BC18529586c513408aB3b`, selector `0xe71e6620` (risc0-ethereum tag v3.1.0, `contracts/deployment.toml`; checked on chain: `verifier()`, `SELECTOR()`, `VERSION()` = "3.1.0"). The wrapper can only stop verification: its guardian, or anyone holding a proof of exploit, can stop it, and it never restarts. It cannot make the vault accept anything new, so Security 8 holds (failsafe L1, TRUST.md section 6). The vault still does not use the `RiscZeroVerifierRouter`, whose owner can add verifiers. Guest `burn_proof` for v3 is built and proven on RISC Zero 3.1.0 in a separate workspace, `guests/burn_proof/r31`, which compiles the same statement source; the LEZ programs stay on 3.0.5 because LEZ v0.4.0-rc3 pins it (a named platform risk). Vaults v1 and v2 held verifier 3.0.0 directly. They are frozen (deposits and releases) and deprecated: `evidence/bridge/README.md`.

Open route (failsafe L2). `releaseOpen(burnSecret, recipient, fee, token, denom, leafIndex, path)` checks the same statement as `burn_proof` with plain SHA-256 in the vault (`burnStatement`), then runs the same checks and payout as `release`. It shares the nullifier map, so one burn pays once across both routes. The recipient and fee are inside the note, so a caller can only pay the bound recipient. It reveals the burn secret, which links this release to its burn: the client offers it only with explicit confirmation, when the zero-knowledge route is stopped or the user chooses it. The freezer can freeze each route separately (`setRouteFreeze`). Foundry tests: `test_openRoute*`, `test_zkRoutePaysAndThenOpenRouteRefuses`, and the Rust vector `test_openRouteStatementMatchesRust`. Measured locally (forge gas report): at most 183,971 gas.

## Chain route: the burn proven from LEZ block bytes

A release on this route takes no burn root from the operator. `VaultChain.releaseChain(seal, journal)` accepts a burn when (1) a RISC Zero proof of guest `burn_chain` shows that a burn transaction for this note is in LEZ block N, that the block does not list it as reverted, and that the blocks from a pinned block P through N to a tip T are one unbroken hash chain, and (2) an anchor says the hash of block T is final. Statement (2) is the one attested input. Today one key, ours, posts it after a fixed delay (`StubLezTipAnchor`, 600 s). It is a plain fact about LEZ that anyone can check with one RPC call (`getBlock(T)`), and it is the place where a Bedrock finality proof or a bonded checkpoint with a challenge window goes in, without changing the guest or the vault.

What an rc3 block commits to (LEZ v0.4.0-rc3 `lez/common/src/block.rs`): `hash = SHA-256(prefix || borsh(block_id, prev_block_hash, timestamp, transactions, reverted))`. So the hash covers the full bytes of every transaction and the list of charged transactions whose action reverted. It does not cover any state (there is no state root), the result of execution beyond the reverted list, the finality status, or any signature (blocks are unsigned).

Guest `burn_chain` (RISC Zero 3.1.0, `guests/burn_chain`, statement in `crates/lez_chain/src/statement.rs`):

- Private input: the burn secret, recipient, fee, token, denomination, chain id, vault, program and instance (as for `burn_proof`), the pinned block's id and hash, the bytes of blocks P+1..T, and the index of the burn block and of the transaction in it.
- Checks: each block's hash recomputed from its bytes, each link to the previous hash, ids rising by one from P. In the burn block, a full parse of every transaction (borsh has no length prefix per transaction, so offsets from the host would let a fake burn hide inside another transaction's data). The transaction at the index carries this burn: for a private burn, a `PrivateToPublic` boundary step whose sender program is the bridge, whose receiver is the instance's accumulator `PDA(bridge, sha256("bridge/acc/v1" || instance))`, and whose message is `Append{instance, token, denom, note, memo}`; for a public burn, a root call to the bridge with `Burn{config, ...}` whose config gives this instance. The note is recomputed from the secret, recipient and fee. The transaction's position is not in `reverted`.
- Journal: version 2, 15 ABI words (480 bytes): the fields of `Vault.BurnJournal` with the accumulator root replaced by `pinnedId, pinnedHash, tipId, tipHash, burnBlockId`.

`VaultChain` is `Vault` plus `releaseChain`, which checks: journal version 2 and length; the pairing (chain id, `vault == this`, program, bound instance); `pinnedId` and `pinnedHash` equal to the vault's immutables; `tipAnchor.isFinalTip(tipId, tipHash)`; the token, denomination and fee; the nullifier unspent in the map shared with `release` and `releaseOpen`; `verifier.verify(seal, chainImageId, sha256(journal))`; then the cap and the same payout. Errors: `WrongPinnedBlock`, `TipNotFinal`, and the existing ones.

**What is proven:** the burn transaction for this note is transaction i of the block with hash H_N; the block does not list it as reverted; its bytes show the bridge program delivering `Append{instance, token, denom, note}` to the instance's accumulator (private burn), or a holder's `Burn` root call to the bridge with a config whose instance is this one (public burn); and blocks P..N..T are one unbroken hash chain with rising ids from the pinned (P, hash).
**What is attested:** that the hash of block T is on the real, final LEZ chain. One input, today posted by our key, accepted 600 s later.
**What is not proven, and stays trusted:** that LEZ executed those blocks correctly, that the attested tip is final, and which chain is the real one. LEZ v0.4.0-rc3 blocks are unsigned and their headers carry no state root, so none of these can be proven from block bytes. "Not reverted" and "sent by the bridge program" rest on the sequencer having run and verified the transaction (the bridge program itself refuses an `Append` from any other sender, `programs/bridge/src/program.rs`). Anyone can build a chain of hashes from the pinned block: the pinned block says which chain is meant, it does not stop a dishonest attester.
**What is no longer trusted:** the operator's own computation of the accumulator root, and any bridge-specific value posted by the operator.

This is one part of stage 2 below (transaction inclusion from block bytes), with the stage 4 stub narrowed from a root to a block hash. It does not rebuild the accumulator root, and it proves neither execution nor finality.

Measured once for real (`evidence/bridge/chain-anchor/sepolia.json`): a private burn on `seq-pub` (dev mode off, block 6588, 255,647 bytes, one privacy-preserving transaction), pinned block 6583, tip 6593, ten blocks hashed. Executor: 1,761,615 user cycles. Groth16 proof on RISC Zero 3.1.0 with segments of 2^18 cycles: 3,209 s (53.5 min) for 16 segments, peak 2.33 GiB in the prover's process tree, on a shared machine (load average 11 on 8 threads); the final Groth16 compression runs in RISC Zero's prover container, outside that tree, and was seen at 4.8 GiB in one sample. On Sepolia through the 3.1.0 emergency stop: `releaseChain` 651,764 gas; a release before the tip's delay refused with `TipNotFinal` and a repeat refused with `AlreadyReleased`, both as mined transactions.

Cost. The proof's cost grows with the distance from the pinned block, because every byte from P+1 to T is read and hashed: about 7 user cycles per input byte (executor, `evidence/bridge/chain-anchor/executor.json`; reading about 3, hashing 1.2 to 4, parsing the burn block 1.6 to 2.5). An idle `seq-pub` block is 711 bytes, about 6,300 cycles; a block with one privacy-preserving transaction is about 256 KB. At one block every 15 s, 30M cycles is reached at about 4,700 idle blocks between P and T (about 19 h). A production anchor therefore moves the start forward: each accepted tip becomes the next pinned block, which is a change to the vault (the pinned block is an immutable in the sprint).

Test arrangement on Sepolia. The burn was already released on vault v4 by the root route (`evidence/bridge/real-burn-v4/`). It is proven again on a test vault bound to the same LEZ instance. The nullifier depends only on the burn secret, but each vault keeps its own nullifier map, so the burn pays once per vault: the test vault holds only the 0.001 ETH deposited to pay this release. A production instance has one vault. Nothing in the guest ties the instance to one vault for a private burn, so a second vault bound to an instance can only pay out of its own deposits.

## Chain route with a 3-of-4 tip anchor (spike, stand-in key set)

`CommitteeLezAnchor` replaces the one-key stub on a second chain-route test vault. It is the milestone-1 quorum spike run early, with signer keys we made for the test, to measure the Ethereum side of a k-of-n check; it is not a quorum of LEZ's sequencers, and it does not meet RFP-022's rule against trusting signers (TRUST.md section 1).

What LEZ v0.4.0-rc3 signs. A block carries no signature: `BlockHeader` is `block_id, prev_block_hash, hash, timestamp` (`lez/common/src/block.rs`). One sequencer, the turn holder, signs the Logos Blockchain transaction that inscribes `borsh(Block)` on the zone's channel, with its Ed25519 channel key (`lez/sequencer/actors/bedrock/src/actor.rs`, `sign_and_submit`). A threshold of the accredited keys signs only channel configuration changes and channel transfers (`lez/sequencer/actors/channel_signing/src/protocol.rs`), with threshold `max(1, min(ceil(2n/3), n-1))`: 1, 1, 2, 3, 4, 4, 5 for n = 1 to 7 (`channel_config_threshold`, `lez/sequencer/core/src/committee_discovery.rs`). Finality (`Pending`, `Safe`, `Finalized`) sits outside the hashed block data and comes from the Logos Blockchain. So no quorum signs a block in rc3, and a quorum checkpoint over the tip would be a new duty for the accredited sequencers. In standalone mode, which `seq-pub` runs, the Bedrock actor is a mock with no channel key and an empty accredited key list (`lez/sequencer/service/src/lib.rs`), and every block reports `Pending`: there is neither a sequencer key nor a finality status to read.

The contract (`contracts/src/CommitteeLezAnchor.sol`) implements both `ILezTipAnchor` and `ILezAnchor`, so a vault that uses it for both routes has no anchor that one key can move. It has no owner and no setter: the signer set (3 to 16 addresses, strictly ascending), the threshold (at least LEZ's rule for that size, at most n), the delay and the zone id are fixed at construction; a new set is a new anchor and a new vault. A signature is EIP-712 over `Tip(bytes32 zone,uint64 blockId,bytes32 blockHash)` or `Root(bytes32 zone,bytes32 lezInstance,bytes32 accumulatorRoot)`, domain `("beacon-veil LEZ anchor", "1", chainId, anchor)`, so it is bound to one anchor on one chain. `attestTip` and `attestRoot` take at least `threshold` signatures from anyone; each must recover (OpenZeppelin `ECDSA.recover`, which refuses malformed and high-s signatures) to a member, in strictly ascending order, so no signer counts twice. The statement is accepted 600 s later. Two different hashes attested at one block id void that height: neither is final. Errors: `BadSignerSet`, `TooFewSignatures`, `NotASigner(address)`, `SignersNotAscending`, `AlreadyAttested`. `Vault.sol`, `VaultChain.sol`, the guest and its image id are unchanged; the new vault needed one new Groth16 proof only because the journal names the vault.

A signer (`tools/bridge_cli/committee_sign.sh`) reads blocks P..T from the sequencer, recomputes each hash from the served bytes, checks each link back to the pinned block, checks its own key is in the set, and only then signs `tipDigest(T, hash)`. It cannot check finality or that the sequencer it reads is the real chain.

**What is proven:** unchanged from the chain route above.
**What is attested:** that block T with hash H is on the real LEZ chain. At least 3 of 4 named keys signed (zone, block id, block hash) for this anchor on this chain. Anyone may post the signatures. The anchor accepts the tip 600 s later. No key can change the set, and the anchor has no owner.
**What is not proven, and stays trusted:** that LEZ executed those blocks correctly; that the tip is final (our standalone sequencer reports every block as `Pending`, so there is no finality to read); and which chain is the real one, now on the word of 3 of 4 keys, not 1. All four keys are ours and all four signers read one sequencer, so today this is still one party. The keys are secp256k1 keys made for this test; LEZ's sequencer keys are Ed25519.
**What is no longer trusted on this vault:** any single key, for which tip or root is final. The vault's admin and freezer is still one key (it can freeze releases, as on every test vault).

Measured on Sepolia (`evidence/bridge/committee-anchor/sepolia.json`): the same burn (block 6588), pinned block 6583 and tip 6593 as above, on a third test vault bound to the same instance, with its own 0.001 ETH deposit. Groth16 2,740 s for 16 segments of 2^18 cycles, peak 2.32 GiB in the prover's process tree, on a shared machine. `attestTip` with 3 signatures, posted by an address that is not a signer: 264,343 gas (the one-key stub's `attestTip` on Sepolia: 130,674 gas, `evidence/bridge/chain-anchor/sepolia.json`). `releaseChain` 654,147 gas. Refused as mined transactions: 2 signatures (`TooFewSignatures`), 2 members and a key outside the set (`NotASigner`), one member twice (`SignersNotAscending`), the tip's signatures submitted for block 6592 (`NotASigner`), the accepted attestation again (`AlreadyAttested`), a release before the delay (`TipNotFinal`) and a repeat release (`AlreadyReleased`). The same flow on an anvil fork with the mock verifier, including signatures made for another anchor: `evidence/bridge/committee-anchor/anvil-fork.json`. On a plain local EVM (Foundry gas report) `attestTip` with 3 or 4 signatures executes in at most 102,226 gas (`evidence/bridge/committee-anchor/gas-report.txt`).

Not built: LEZ's own keys, Ed25519 checks (in the vault they would need third-party library code, with no precompile we could rely on; in the guest they would need a new guest, a new image id and a much longer proof), the check inside the proof, key-set changes, a certificate from a channel key, bonding or slashing, and anything that acts on the delay. Each is milestone work.

## Stages of the anchor

| Stage | What makes `isFinal` true | Sprint | Residual trust |
|---|---|---|---|
| 1 | The guest proves membership; the root comes from the anchor | Built: guest, Groth16 path, vault checks | None for the membership statement |
| 2 | Accumulator root rebuilt from LEZ block bytes (rc3 blocks list reverted transactions, `lez/common/src/block.rs:59-67`), inclusion in a Bedrock block k deep | **One part built:** the burn transaction in an unbroken hash chain from a pinned block to a tip, not reverted (guest `burn_chain`, `VaultChain.releaseChain`, one real release on Sepolia). Not built: the accumulator root from block bytes, inclusion in a Bedrock block | That LEZ executed the blocks correctly, and the tip (stage 4) |
| 3 | q distinct sequencer keys built on that block | Built with a stand-in key set: `CommitteeLezAnchor`, 3 of 4 keys of ours, checked in the contract; one release on Sepolia. Not built: LEZ's own keys, Ed25519, the check inside the proof, set changes | q colluding signers; in the sprint all four keys are ours, so one party |
| 4 | Optimistic Bedrock checkpoint: bonded poster, challenge window, slashable equivocation; the window is also the release delay; caps bound loss per window | **Stub only:** `StubLezAnchor`, owner-posted root, fixed delay; on the chain route narrowed to one LEZ block hash (`StubLezTipAnchor`), which anyone can compare with `getBlock` | One honest watcher in the window |
| 5 | Disputes go to a named arbiter set | Design only | The arbiter set |
| later | Stage 2: a zkVM dispute proof replaces the arbiter. Stage 3: direct verification once LEZ headers commit to state | Proposal text | |

F7 asks for no trust in signers. That cannot be met on LEZ v0.4.0-rc3: headers carry no state root, and public execution is not proven. The proposal says so and words the milestone as "stage-1 anchor agreed at M0".

## Recovery (F9)

`bridge_cli recover --lez` reads the accumulator count and every leaf record. For j = 0, 1, ... it derives the burn secret, opens each memo, and checks the note. It reports each own burn with its recipient and fee, and whether the vault already spent its nullifier. `bridge_cli prove --index j` rebuilds the witness from the same data.

## CLI flow (anvil or Sepolia)

```
bridge_cli burn --index 0 --holder <private account> --token eth --denom 10000000000000000 --recipient 0x...
bridge_cli accumulator-root                              # root and count
forge script script/Deploy.s.sol:PostRoot ...            # STUB anchor: owner posts the root
bridge_cli prove --index 0 --out out/burn0 --mode execute    # dev: executor, cycles, mock seal for anvil
bridge_cli prove --index 0 --out out/burn0 --mode groth16    # proving window only: real seal
forge script script/Deploy.s.sol:ReleaseOne ...           # env SEAL, JOURNAL (hex from prove)
```
