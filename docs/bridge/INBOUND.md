# Inbound: Ethereum deposit to LEZ mint

Status: spike (sprint 2026-11-12 to 2026-11-15). Code: `contracts/src/Vault.sol`, `programs/bridge`, `tools/bridge_cli`.
Requirement ids refer to RFP-022.

## Summary

1. The user derives a deposit secret from the wallet seed and computes a commitment that hides the LEZ destination.
2. The user deposits one fixed denomination into the vault with that commitment. The vault appends a leaf to a SHA-256 Merkle tree and stores the new root in a fixed storage slot.
3. Anyone attests "the vault's root slot holds root R at a finalised header" through RFP-021 (`attest`). The statement lands in a root receipt account on LEZ that every claimant against R shares.
4. The user claims privately: one LEZ transaction, proven on the user's machine, shows that some leaf under R opens to a commitment bound to the destination, marks a nullifier, and credits the wrapped balance.

## Hashes (wire format)

All SHA-256. `word(x)` is a 32-byte big-endian integer, as Solidity `uint256`. Addresses are 20 bytes.

| Value | Definition | Computed by |
|---|---|---|
| bridge key | `sha256("wb/key/v1" \|\| bip39_seed(mnemonic, ""))` | wallet (same BIP-39 seed the LEZ wallet uses) |
| deposit secret i | `sha256("wb/dep/v1" \|\| key \|\| chain_id_le8 \|\| vault \|\| i_le8)` | wallet |
| destination binding | `sha256("wb/dest/v1" \|\| bridge_instance \|\| dest_account)` | wallet, program |
| commitment | `sha256("wb/c/v1" \|\| secret \|\| binding \|\| token \|\| word(denom))` | wallet, program |
| leaf | `sha256("wb/leaf/v1" \|\| token \|\| word(denom) \|\| commitment)` | vault, program |
| deposit nullifier | `sha256("wb/n/v1" \|\| secret)` | program |

The vault computes the leaf itself from the token and amount it really received. So a leaf can never claim more than was escrowed. The tree is the Ethereum deposit contract's incremental tree (CC0), depth 32, without the length mix-in. `programs/bridge/src/tree.rs` and `Vault.sol` produce the same roots (Foundry test `test_manyDepositsMatchReference` and the cross-language vectors).

## Vault (Ethereum)

- `deposit(token, denomIndex, commitment)`: token registered, denomination from the token's fixed list (F8), not frozen, under the window cap (F11 stub). ETH: `msg.value` must equal the denomination (F1, no WETH step). ERC-20: `SafeERC20.safeTransferFrom` (F17, USDT-style tokens that return nothing), and the received balance delta must equal the denomination, so fee-on-transfer tokens are refused.
- The root lives at slot `keccak256("beacon-veil.vault.depositRoot") - 1` = `0xc48ce8c9c69c089f8e403bb82c6c6a3b0b082f197e29f63d5843b5b37b73a200`. It is fixed by hand, not by the compiler's layout, so the storage proof does not depend on the build.
- Event `Deposit(token, denom, commitment, leafIndex, root)`. No argument, event or state names the destination (F2, Privacy 3).

## Bridge instance (LEZ)

`BridgeConfig { chain_id, vault, attest_program, attest_instance, tokens[{token, denoms, mint_fee, burn_fee}], treasury }`.
Instance id = `PDA(bridge, sha256("bridge/instance/v1" || borsh(config)))`. Every message carries the config. The program re-hashes it, so the config cannot change after the fact. Every account the bridge writes is keyed by the instance. A second entity with its own config gets disjoint accounts and its own wrapped balances (F16). The config names one chain id and one vault, and the vault binds the instance id once (`bindLezInstance`). That gives the two-way pairing of F15.

## Claim, two transactions

### Step 1: public attestation of the root (anyone)

`bridge_cli attest-root --slot S` reads the header record of slot S from the followed `attest` instance. It builds a storage proof of the vault's root slot at that execution block with `proofgen` (on gloas the proof carries the RLP execution header, checked against `exec_hash`). It then sends `attest.Attest{instance, record, claim, predicate: Eq(R), deliver_to}` as a public transaction.

`deliver_to` is the root receipt: `PDA(bridge, sha256("bridge/root/v1" || sender_program || attest_instance || stmt_id))`. Here `stmt_id = statement_id_state(chain_id, vault, DEPOSIT_ROOT_SLOT, R)`. When a `Statement` arrives from another program, the bridge recomputes this seed from the actual sender program and the statement's own instance and id, and refuses any other receiver. So:

- A fake "attest" program can only write receipts keyed by its own program id. No bridge config points at those.
- A fake light-client instance can only write receipts keyed by its own instance id.
- The real attest program, for the real instance, can only write the receipt of exactly this (chain, vault, slot, R). Any such statement is true, so a first writer cannot squat it with something wrong.

This lets the bridge take the attest program id from its config instead of a build-time constant (the consumer rule). The safety is the same, because the instance id commits to the config.

Privacy of step 1: R is public on Ethereum. It is the root after some deposit, and every deposit before it is under R. The step names no deposit and no destination. It is shared: one attestation serves every claimant against R. A relayer or the bridge operator can run it for everyone. A gloas record attests the parent execution block of the finalised beacon block (see TRUST.md), so a deposit becomes provable one record later.

### Step 2: private claim (the user)

`bridge_cli claim --index i --dest D --root R` rebuilds the leaf list from the vault's `Deposit` logs up to the log whose root is R. It finds the user's leaf by recomputing the commitment, builds the path, and sends one privacy-preserving transaction. The root actor is the claimer's private account under `bridge`, with message `Claim{config, token, denom, secret, dest, leaf_index, path, root}`. The handler:

1. checks the token and denomination against the config;
2. recomputes binding, commitment and leaf, and the root from the path (`ESA:E07` if wrong);
3. sends `CheckRoot` to the root receipt (`WB:B03` unless it holds the statement "vault slot = R" under the configured attest instance);
4. sends `Nullify` to `PDA(bridge, sha256("bridge/null/v1" || bridge_instance || deposit_nullifier))`. It panics `ESA:E11` unless the account is empty, then marks it (R3). The instance is in the seed, so a fake instance cannot mark a real instance's nullifier (griefing found in review);
5. credits `denom - mint_fee` to the destination's holding, `(instance, token) -> amount`, and the fee to the treasury (F13).

The claim's proof runs on the user's machine. On chain one sees: the receipt read (that is, R), a fresh nullifier, and, only for a public destination, the credit. The secret, the leaf index, the path and the commitment never leave the device (Privacy 4). A failed step aborts the whole transaction. Nothing is consumed and the deposit stays claimable (R1).

Uniqueness key. F3 asks for uniqueness "keyed on the statement identifier". A storage statement about the root is shared by every deposit under R, so it cannot be the only key. The key is the per-deposit nullifier, scoped by the bridge instance. Decision-log entry: DECISIONS.md, "claim nullifier".

### Destinations (F4, U6)

- Default: the destination is the claimer's own private account. Nothing about the destination is public.
- `--public`: the destination is a public account and the claimer is a separate private account. The claim is still zero-knowledge: the opening is proven privately. Only the credit to the public account is visible: token, amount, time.

### Why the claim must run privately

If a claim ran on a public account, the sequencer would see the secret and the path, which links the deposit to the mint. `bridge_cli` only sends claims as privacy-preserving transactions. The program cannot tell its placement, so this is a client rule. It is stated in TRUST.md.

### Local test source (dev only)

`programs/bridge/src/bin/bridge_dev_source.rs` forwards any statement unchecked. It exists only for the local round trip (`tools/bridge_cli/e2e_local.sh`) against an anvil vault, which no light client follows. It is safe to exist because of the receipt seed above: it can only write receipts keyed by its own program id. Only a config that names it as `attest_program` reads those, and the e2e config does so on purpose. A real deployment's config names the real `attest` program. The Sepolia run uses `attest-root` through RFP-021. Nothing selects it by default: a config must name its attest program explicitly. `bridge_cli` refuses any config on a chain other than 31337 whose attest program is the deployed dev source (`deploy/ids.toml` key `bridge_dev_source`). `dev-attest-root` runs only on chain 31337.

## What was shown, and how (2026-11-12)

- The claim was shown in dev mode on Sepolia data. Against the follower's Sepolia instances (run A on vault v2, run B on vault v3), `attest-root` proved the vault's root slot through RFP-021. The private `Claim` then landed on seq-dev in dev mode (`RISC0_DEV_MODE=1`, no real proof), and a repeat claim was refused with `ESA:E11_ALREADY_CONSUMED`. Evidence: `evidence/bridge/k9-part2-runA-v2/`, `evidence/bridge/k9-part2-runB-v3/`.
- On 2026-11-12 no real proof of a private bridge claim was made. It was made on 2026-11-21 (next section).
- The mechanism the private claim relies on was shown with a real proof by G3: a private `Attest` against a light-client record that was itself proven for real (the G3 consumer, about 35 min and 9.2 GiB peak on this laptop; `evidence/g3/real-proof.json`, raw files under `evidence/prove/g3-real-consumer/`). The bridge claim uses the same placement: a private actor, public PDAs reached by sync messages, and a statement checked against a record.

## The claim with a real proof (2026-11-21)

- Vault v4 on Sepolia (`evidence/bridge/sepolia-v4.json`) names the bridge program on `seq-pub`, a standalone sequencer started with `RISC0_DEV_MODE` unset. A 0.001 ETH deposit (block 12,176,877), then `attest-root` against the Sepolia instance on `seq-pub` (record slot 11,627,968), then the private `Claim`, proven for real: 1,856.03 s, peak 9.189 GiB, block 6467, witness set 239,202 bytes. The claimer's wrapped balance moved by exactly the denomination. The same claim again was refused with `ESA:E11` after 1.87 s, during execution, before any proof. Evidence: `evidence/bridge/real-claim-v4/`.
- Limits: the record the claim checked was written by that instance through the public path (three public transactions per update, executed by the sequencer, no proof; `docs/PUBLIC-PATH.md`). `seq-pub` is not a public Logos testnet. A claim to a public destination was not run.

## Recovery (F9, R5)

`bridge_cli recover --dest <own accounts>` needs only the mnemonic and chain state. For i = 0, 1, 2, ... it derives the deposit secret and tests every `Deposit` log's commitment against each of the wallet's own accounts as destination. It stops after 20 indexes with no match (gap limit). For each match it reads the nullifier account on LEZ to report claimed or open. No secret is generated at deposit time, and no local state is needed.

## What is not built in the sprint

Registry administration through RFP-001 (tokens are fixed per config), rolling global caps (per-token fixed windows only), the LEZ-side freeze through RFP-002, relayer fees on the inbound leg, LP-0013 token-program mints (the wrapped balance is a bridge-held ledger per instance), and decimal conversion (identity: the wrapped base unit equals the token's base unit, so it is exact). These are milestone work.
