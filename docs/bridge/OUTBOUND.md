# Outbound: LEZ burn to Ethereum release

Status: spike. Code: `programs/bridge` (Burn, accumulator), `guests/burn_proof`, `contracts/src/Vault.sol` (`release`), `contracts/src/StubLezAnchor.sol`, `tools/bridge_cli`. Design source: PLAN.md 2.4.

## Summary

1. The holder burns one denomination of the wrapped token on LEZ, from a private (default) or public account (F6). The burn appends a leaf to the instance's burn accumulator. The leaf hides the Ethereum recipient (F5).
2. An anchor tells the vault that an accumulator root is final. In the sprint this is `StubLezAnchor`: the owner posts the root. That is a stated trust assumption (TRUST.md).
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

Verifier choice. The vault holds the RISC Zero Groth16 verifier 3.0.0 directly as an immutable: Sepolia `0x2a098988600d87650Fb061FfAff08B97149Fa84D`, selector `0x73c457ba`, from `lib/risc0-ethereum/contracts/deployment.toml` at v3.0.1. It does not use the `RiscZeroVerifierRouter`, whose owner can add or remove verifiers. Security 8 says no party may change what the bridge accepts after deployment. A new proof system means a new vault and a migration (see TRUST.md). This deviates from PLAN.md 2.4's "through RISC Zero's verifier router".

## Stages of the anchor (PLAN.md 2.4)

| Stage | What makes `isFinal` true | Sprint | Residual trust |
|---|---|---|---|
| 1 | The guest proves membership; the root comes from the anchor | Built: guest, Groth16 path, vault checks | None for the membership statement |
| 2 | Accumulator root rebuilt from LEZ block bytes (rc3 blocks list reverted transactions, `lez/common/src/block.rs:59-67`), inclusion in a Bedrock block k deep | Design only | None |
| 3 | q distinct sequencer keys built on that block | Design only | q colluding sequencers |
| 4 | Optimistic Bedrock checkpoint: bonded poster, challenge window, slashable equivocation; the window is also the release delay; caps bound loss per window | **Stub only:** `StubLezAnchor`, owner-posted, fixed delay | One honest watcher in the window |
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
