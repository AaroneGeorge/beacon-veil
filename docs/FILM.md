# Film script and shot list

Narrator: Aarone George. Target length: 4 minutes 30 seconds, hard limit 5 minutes. Recording: after the proposal text is final. The film is linked from both proposals.

Status: script written 2026-11-12, figures filled in from `evidence/` on 2026-11-17. This repository holds the script only; no recording is included. The narrator is expected to rewrite the lines in his own words before recording. Narration is plain and says only what the screen shows.

Rules for the recording:

- Show a clean clone, not the working directory of the build machine. Run the commands of `README.md` section 1.
- Terminal font 18 pt or larger, dark text on a light background, 1920 by 1080, 30 fps. Mouse cursor visible.
- Never show a key, a wallet password, a token or a Sepolia private key. Environment variables that hold them are not printed. Blur any wallet file name.
- Do not speed up a command that proves. If a proof takes hours, show the finished output file and say it was produced earlier, with its timestamp.
- Say "measured" only for a figure that appears on screen from a file in `evidence/`.
- No music with lyrics. A quiet track under the narration is fine.

## Shot list

| # | Time | On screen | Source and prerequisite |
|---|---|---|---|
| 1 | 0:00 to 0:20 | Title card with the project name, the two RFP numbers, the repository URL. Then Aarone's GitHub profile for two seconds | Project name beacon-veil, `https://github.com/AaroneGeorge/beacon-veil` |
| 2 | 0:20 to 0:55 | Slide with three boxes: Ethereum header, inclusion proof, fabricated chain. A red cross on "valid inclusion proof against a fake root" | Drawn by hand. Based on RFP-021 Design Rationale |
| 3 | 0:55 to 1:30 | `docs/REUSE.md` section 1 table, then the `[patch.crates-io]` block of `Cargo.toml` with the `ethereum_hashing` pin highlighted | `docs/REUSE.md`, root `Cargo.toml` |
| 4 | 1:30 to 2:20 | Terminal: `make ci` result lines, then the chain test: `Init`, three updates, one handoff, with the head slot and period printed after each. Then the table of cycles per update | `README.md` step 6, `evidence/cost/cost.md`, `evidence/g1/chain.json`, `evidence/g1/real-proof.json`. Prerequisite: `tests/e2e_chain` green on a sequencer |
| 5 | 2:20 to 2:50 | Terminal: `cargo test -p adversarial`, seven tests with their names and the error code each one asserts. Zoom on `vs1_fabricated_chain` | `tests/adversarial`. Prerequisite: on-chain legs pass |
| 6 | 2:50 to 3:20 | Terminal: consumer test. First transaction accepted, consumer state changed. Same transaction again, rejected with `ESA:E11` | `scripts/gate/g3_consumer.sh`, `evidence/g3/run.json` |
| 7 | 3:20 to 3:45 | The follower log: first and last line of run B (34.49 h), the `beacon_source` field highlighted, the checker's exit code | `evidence/g4/run-b/follower.jsonl`, `evidence/g4/run-b/g4-summary.json` (run A: `evidence/g4/run-a/`) |
| 8 | 3:45 to 4:20 | Browser: the vault on a Sepolia explorer, one deposit transaction (the event shows a commitment and no destination). Terminal: the claim, the second claim refused. File listing: the Groth16 proof and the gas figure | `docs/bridge/INBOUND.md`, `evidence/` bridge files. Prerequisite: Sepolia deployment and a claim run |
| 9 | 4:20 to 4:30 | Text on screen: three gaps (anchor stub, standalone sequencer, no audit). Then contact and the price line | `README.md` section 6 |

If shot 7 or 8 has no evidence when recording starts, cut it and cut the matching narration. Do not substitute a mock.

## Narration

Lines are numbered by shot. Approximate reading time at 140 words a minute is in brackets.

**Shot 1 [0:15]**
I am Aarone George. This is beacon-veil, our proposal for Logos RFP-021 and RFP-022. We built a working prototype before applying, so you can check what we say. Everything I show comes from a clean clone of the public repository.

**Shot 2 [0:30]**
A LEZ program that reacts to an Ethereum event needs to know the event is real. An inclusion proof alone does not tell it. Anyone can build a fake chain and a valid proof against it. The program has to check that the header belongs to the real, finalised Ethereum chain. That means following the sync committee from a checkpoint.

**Shot 3 [0:30]**
We did not write that light client. It is Helios, version 0.12.0, used without changes, running inside a RISC Zero guest. Helios itself has not been audited, and we say that too. The reuse document lists what we took and under which licence, and what is ours. What is ours is the instance program, the inclusion checks and the glue. That is what an auditor needs to read.

**Shot 4 [0:45]**
This is the chain test on a local LEZ sequencer, version 0.4.0 release candidate 3. It initialises an instance from a real mainnet checkpoint, applies three updates, and crosses a sync-committee handoff. Each update is real mainnet data. One update costs between 46 and 65 million cycles. The per-transaction cap is 10 million. So the update is proven privately and applied publicly. We proved one for real: it took five and a half hours on this laptop and used about nine gigabytes of memory. Those are measured numbers. Our first run was higher, and the cause was our own glue code, which we fixed without touching Helios. We say so in the proposal.

**Shot 5 [0:30]**
These are the adversarial tests. A fabricated chain with its own committee and valid proofs is rejected. So are a committee that was never handed off, too few signatures, a header that is not finalised, tampered roots, the wrong chain and a predicate the proof does not cover. Each test asserts an exact error code. We found while writing them that the Helios verify function does not enforce the two-thirds quorum by itself. We check it before calling Helios.

**Shot 6 [0:30]**
A consumer program takes an attestation of an ERC-20 transfer and acts on it once. The same transaction sent again is refused. The nullifier is scoped to the consumer's own instance. Without that, someone else's instance could burn the identifier of a real transfer before the honest user acts.

**Shot 7 [0:25]**
This is the follower. The primary run lasted 34 and a half hours against the Sepolia testnet without anyone touching it, and a second run lasted almost 36. Its beacon data came from the Logos beacon module. Every submission was applied, 22 of 22, and the run crossed one sync-committee handoff. The proofs in these runs were in development mode, so this shows the loop and not the proving time. The log has one process id and no gap larger than the limit.

**Shot 8 [0:35]**
The bridge is a spike. This vault is deployed on Sepolia. A deposit carries a commitment, and nothing on Ethereum names the destination. The claim is one private LEZ transaction against a root attested through the same light client. A second claim is refused. The release side verifies a Groth16 proof of a burn. Its anchor is a stub that the repository owner controls. We built the stub to test the rest, and we name it as a stub. The proposal explains the stages that replace it.

**Shot 9 [0:10]**
We have not shipped on LEZ before this work. There is no audit yet, the audit is a separate allowance paid by Logos. Contact details and the price are in the proposals. Thank you.

## Check before recording

- [ ] Every figure read aloud appears on screen from a file.
- [ ] The commit hash of the clean clone is shown once.
- [ ] No secret on screen, in the shell history or in a window title.
- [ ] Shots 7 and 8 have evidence, or the lines are cut.
- [ ] Total time at the recorded pace is under 5 minutes.
