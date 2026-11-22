# Bridge trust model and privacy analysis (spike)

This file states what the RFP-022 spike trusts, what it reveals, and what is not yet built. Requirement ids refer to RFP-022. Where the sprint uses a stub, the stub is named.

## 1. What a user relies on

| Leg | Relied on | Not relied on |
|---|---|---|
| Inbound (deposit to mint) | RISC Zero proof system (LEZ privacy circuit); Ethereum consensus through RFP-021: the sync committee's honest supermajority (Helios 0.12.0, unmodified); correctness of `attest`, `attest_inclusion` and `bridge`; the token issuer | Any relayer, prover or operator. Step 1 (`attest-root`) is permissionless and shared; the claim is proven on the user's device |
| Outbound (burn to release) | RISC Zero Groth16 (verifier 3.1.0 behind its emergency stop, immutable; see section 6), or the open route, which needs no proof system; correctness of `burn_proof`, `bridge` and `Vault`; **the anchor**: in the sprint, `StubLezAnchor`, an **owner-posted stub** whose owner key can make any root final; the token issuer | Any relayer: the recipient and the fee are bound inside the proof |
| Outbound, chain route (`VaultChain.releaseChain`) | RISC Zero Groth16 (verifier 3.1.0 behind its emergency stop); correctness of `burn_chain`, `crates/lez_chain`, `bridge` and `VaultChain`; **the tip anchor**: in the sprint, `StubLezTipAnchor`, an **owner-attested stub** whose owner key can make any block hash final, including one of a chain the owner built from the pinned block; that LEZ executed the blocks correctly; the token issuer | Any relayer; the operator's computation of the accumulator root, and any bridge-specific value posted by the operator |

**The stub is a stub.** `StubLezAnchor` stands in for the stage-1 optimistic Bedrock anchor of OUTBOUND.md. With it, whoever holds the owner key can release any registered asset up to the window cap. Nothing in the sprint claims trustless release. F7 cannot be met on LEZ v0.4.0-rc3: headers carry no state root, and public execution is not proven. The stated residual trust of the grant's stage 1 is: canonical Bedrock header, attested optimistically, with an arbiter on dispute.

**The chain route narrows the stub's statement, it does not remove it.** On `releaseChain` (OUTBOUND.md, "Chain route") a RISC Zero proof shows that a burn transaction for this note is in LEZ block N, that the block does not list it as reverted, and that blocks P (pinned in the vault) through N to a tip T are one unbroken hash chain. The one attested input is "the hash of block T is on the real, final LEZ chain", posted by the stub's owner key and accepted after 600 s. Anyone can check it with one `getBlock(T)` call. Still trusted, because LEZ v0.4.0-rc3 blocks are unsigned and their headers carry no state root: that the attested tip is on the real, final chain, and that LEZ executed that chain correctly. The pinned block says which chain is meant; it does not stop a dishonest attester, since anyone can build a chain of hashes from it. No longer trusted on this route: the operator's own computation of the accumulator root. Proven for real once, for one burn on `seq-pub`, on a test vault (`evidence/bridge/chain-anchor/sepolia.json`).

## 2. Ethereum finality under gloas (Sepolia)

Sepolia runs gloas (ePBS). A gloas light-client header proves `bid.parent_block_hash` (generalized index 2856), and Helios 0.12.0 uses the same index. The beacon chain advances `latest_block_hash` only when a payload was delivered and verified. Fork choice rejects a child that declares a withheld parent FULL. So a light-client-proven execution hash is canonical under the same assumption as before gloas: an honest sync-committee supermajority. Source: consensus-specs master `c3e16117`, checked on 2026-11-12.

Two consequences:

- A gloas record attests the parent execution block of the finalised beacon block. It lags by at least one slot. A deposit becomes claimable one record later than its block's own finalisation.
- A gloas record holds only `exec_hash`; its state and receipts roots are zero. The storage proof of the vault's root then carries the RLP execution header. `attest_inclusion` checks it by keccak against `exec_hash`: `ESA:E01` if the header is missing when the roots are zero, `ESA:E15` if the hash does not match.

Finality depth (F12): a deposit is claimable only against a header the light client has applied as finalised (two epochs, about 13 minutes on mainnet). The depth is not configurable per bridge in the sprint. It is the attest instance's rule. A bridge that wants a deeper margin would require `record.slot + k <= head.slot`. That is milestone work.

## 3. Off-chain participants

| Participant | Role | Can it block? | Can it steal, redirect, deanonymise? |
|---|---|---|---|
| Root attester (`attest-root`) | Posts "vault root = R at header H" for everyone | No: anyone can run it, including the user | No: the statement is checked on chain; it names no deposit |
| Claim prover | The user's own device | n/a | n/a |
| Burn prover (`burn_proof`) | Anyone with the witness, normally the user | No: anyone can prove | A prover who gets the witness learns the burn's recipient. It cannot change the recipient or the fee (bound in the note) |
| Release relayer | Calls `release`, receives `fee` | No: anyone can call | No: the recipient and fee are in the proof. It learns the link only if the user gives it the witness instead of the seal |
| STUB anchor owner | Posts accumulator roots | **Yes, in the sprint** | **Yes, in the sprint**: see section 1 |
| STUB tip anchor owner (chain route) | Attests one LEZ block (id, hash) | **Yes, in the sprint** | **Yes, in the sprint**: it can attest the tip of a chain it built from the pinned block, with a fake burn in it; anyone can see the attested hash differ from `getBlock` during the 600 s delay, but nothing in the sprint acts on that |
| Follower (RFP-021) | Keeps the attest instance current | No: anyone can submit updates | No |

## 4. Privacy (Privacy 1 to 7)

What each step makes public:

| Step | Public | Hidden |
|---|---|---|
| Deposit (Ethereum) | depositor address, token, denomination, time, commitment, leaf index, new root | the LEZ destination (bound inside the commitment) |
| Attest root (LEZ, public) | a vault root R, the header record used | which deposit, who will claim |
| Claim (LEZ, private) | that R was used, one fresh nullifier, the time; for a public destination, the credit (token, amount) | which leaf, the commitment, the destination if private |
| Burn (LEZ) | token, denomination, time, note, memo; for a public holder, the holder | the Ethereum recipient and the fee; the holder if private |
| Release (Ethereum) | recipient, token, denomination, fee, nullifier, accumulator root, relayer | which burn (any leaf under the root) |

Remaining correlations, and why they remain:

- **Amount and token.** Public on both chains by construction. Fixed denominations (F8) put every deposit of one (token, denomination) into one anonymity set. Set size: the deposits of that pair under the root used, minus those already claimed. The CLI prints this leak at each step (U5).
- **Timing.** A claim against root R shows the deposit came no later than R. Using the latest attested root makes the set as large as possible. The UI should advise waiting (U5). Claims never expire (R4): the nullifier account stays empty until used, and any later root still contains the leaf.
- **Fee payer.** A claim is a privacy-preserving LEZ transaction paid from the claimer's private account, so no public funding source shows. The release fee goes to whoever calls `release`. The recipient need not hold ETH first.
- **Errors.** Error codes name the check that failed, never the deposit or burn (Privacy 5, U8). A repeat claim and a claim against an unattested root both fail before anything is consumed.
- **Network level.** IP correlation between the Ethereum deposit and the LEZ claim is out of scope and is residual leakage (Privacy 6).

Client rule: a claim must run as a privacy-preserving transaction. The program cannot see its own placement. A claim on a public actor would publish the secret and the path, which links deposit and mint. `bridge_cli` sends claims only as private transactions. A public destination is reached by a separate private claimer (U6: the private destination is the default).

## 5. Soundness of supply (Security 6)

- Mint: needs a leaf under an attested vault root (the vault computes the leaf from what it received) and an empty nullifier account. One deposit gives one nullifier, so it can be minted once. The nullifier account is `PDA(bridge, sha256("bridge/null/v1" || bridge_instance || deposit_nullifier))`. The instance id is in the seed, so another instance (another entity's config, or a fake one) cannot mark this instance's nullifiers. Without that, a fake instance could front-run a real claim and burn its nullifier (griefing found in review, 2026-11-12).
- Root receipts: written only by a statement whose real sender program, attest instance and statement id derive the receipt's address. A fake attest program or a fake light-client instance writes only to receipts that no honest config reads.
- Wrapped balances are kept per (instance, token). Tokens minted by a fake instance are a different asset and cannot be burned against this instance's vault.
- Release: needs a valid Groth16 proof for the fixed image id, an anchor-final root, the pairing checks, and an unspent nullifier. Each burn leaf has one secret and therefore one nullifier. On the chain route (`VaultChain.releaseChain`): a valid Groth16 proof for the fixed `burn_chain` image id, the vault's pinned block, a tip final in the tip anchor, the pairing checks, and an unspent nullifier in the map shared with the other two routes. The nullifier depends only on the burn secret, so in one vault a burn pays once across all three routes. Each vault keeps its own map: two vaults bound to one instance (as on Sepolia, vault v4 and the chain-route test vault) would each pay the same burn once, each from its own deposits. A production instance has one vault.

## 6. Freeze, caps, migration, failsafe

- Freeze (F14, Security 5): the vault has global and per-token switches for deposits and releases (`FREEZER_ROLE`). The LEZ-side freeze (RFP-002) is not built in the sprint. The two halves are independent by construction. The freeze works per token and per protocol only: the protocol does not know whose holdings are whose. That is a designed consequence of the privacy, not a gap.
- Caps (F11, Security 4): per token, a fixed window, separately for deposits and releases. The stub is not rolling, and there is no global cross-token cap (it would need a common unit). Milestone M4.
- No swappable verification (Security 8): the verifier, the burn image id, the anchor, the LEZ program id and the bound instance are immutable or write-once in the vault. On LEZ, `bridge` is deployed with the immutable flag. Migration means a new vault and a new instance. In-flight deposits are claimed on the old instance (it stays live). In-flight burns are released by the old vault. A freeze of new deposits on the old vault moves users over. The vault holds the RISC Zero verifier through its emergency-stop wrapper (an immutable address). The wrapper can only stop verification, never change it. RISC Zero advisory GHSA-g6qg-9j8p-7j87 (2026-11-11) affects verifier 3.0.0 and `risc0-zkvm` below 3.1.0. Vault v3 uses verifier 3.1.0 (Sepolia verifier `0x8c915b5071206C6E328BC18529586c513408aB3b`, emergency stop `0xA1DCB16cb765b3CA71605339F0C37Bfe0F60bA29`). Vaults v1 and v2 use verifier 3.0.0 and are deprecated testnet artefacts.
- Failsafe (Security 9). A freeze stops the broken component. Each in-flight position then has an exit that does not use that component. The owner claims it with the same seed-derived secret as an ordinary claim or release, and it pays only the destination bound in that secret. A cross-chain fact comes from Ethereum through RFP-021, or else it carries the anchor's trust, and we say which. Layers: (L1) the vault calls the RISC Zero verifier through its emergency-stop wrapper, which anyone can stop with a proof of exploit and which can never be restarted; (L2) an open release route checks the same statement as `burn_proof` with plain SHA-256 in the vault, so holders can exit if the proof system breaks, at the cost of revealing their own link; (L3) a burn that cannot be released is voided on Ethereum and re-credited on LEZ, which reads the void through RFP-021; (L4) an unclaimed deposit can be cancelled on LEZ into a release; (L5) after a permanent mint latch, an unclaimed deposit can be refunded on Ethereum against the anchored set of claimed nullifiers. L1 and an L2 stub are in the sprint. L3 and L4 are M4 work. L5 is designed at M4 and built at M6 only if the stage-1 anchor is agreed at M0. A refund on Ethereum without the anchor is not offered: it would pay a deposit twice, because the vault cannot see LEZ claims. If the anchor itself fails, the escrow has no exit, and we do not invent one. The full matrix is in `research/10-failsafe-and-helios-audit.md` section A5.

In vault v3 (built): L1 is the `verifier` immutable set to the 3.1.0 emergency-stop address. L2 is `releaseOpen` with `burnStatement`, and `setRouteFreeze(zkRoute, openRoute)` freezes each route separately. Both routes share `burnNullifierSpent`.

## 7. Open issues (2026-11-12)

- The outbound anchor is a stub (section 1). Stages 3 to 5 are design text. Of stage 2, one part is built: the burn transaction proven in an LEZ hash chain from block bytes (the chain route); the accumulator root from block bytes and inclusion in a Bedrock block are not. The chain route's pinned block is an immutable, so the proof's cost grows with the distance from it (OUTBOUND.md, "Chain route").
- Burn leaf records are a stand-in for LP-0012 events through the indexer (OUTBOUND.md). Concurrent burns may need a retry.
- The wrapped balance is a ledger inside `bridge`, not an LP-0013 token mint. ATA compatibility (RFP-022 dependency text) is milestone work.
- Finality depth per bridge is not configurable (section 2).
- RISC Zero advisory GHSA-g6qg-9j8p-7j87: LEZ v0.4.0-rc3 pins `risc0-zkvm` 3.0.5, which is affected. The inbound claim and RFP-021 Path A depend on rc3's receipt checks until LEZ moves to 3.1.0.
