# The public update path on LEZ testnet 0.3

Status: written 2026-11-22. **Simulated against the public testnet, not yet sent.** The transactions below have been
built, signed and run in LEZ v0.3.2's own state machine seeded with the accounts read from the testnet; none has been
sent, because the paying account holds no testnet funds yet. This page is updated with the transaction hashes when the
run is sent; until then nothing here says the programs are deployed on a public network.

## The network

LEZ testnet 0.3 (`https://testnet.lez.logos.co`, LEZ release `v0.3.2`) is the only public LEZ network with an
endpoint; testnet 0.4 has none yet (Logos's table "Testnet endpoints", read 2026-11-21). The delivered programs target
LEZ v0.4.0-rc3, whose program model (actors) differs from 0.3's (`plan` + `apply`).

## What runs there

`lez-v03/` holds two thin 0.3 wrappers around the same verifier code, compiled unchanged (`lez-v03/README.md`,
`docs/DECISIONS.md` D-60):

- `lc_verify_v3_v03`: one update as `Begin`, `Miller`, `Finish`, through `attest_core::lc_v3::handle`.
- `attest_v03`: `Init` and `Apply`, through `attest_core::rules::init` and `rules::apply`; `Apply` only as
  `Finish`'s chained call, with the caller checked by rule E12 as on rc3.

The 0.4 programs remain the deliverable.

## Measured in simulation

In `lee::V03State` (LEZ v0.3.2's action phase: `plan`, every `apply` and the chained call summed), with locally built
guests, on the committed mainnet, Sepolia and Hoodi fixtures and the 342-signer floor chain
(`lez-v03/harness/tests/cap.rs`):

| Step | Largest, cycles | Note |
|---|---|---|
| `Begin` | 6,906,533 | the 342-signer floor; 4,459,873 on a real Sepolia update |
| `Miller` | 5,786,289 | |
| `Finish` | 4,472,634 | with the chained `attest_v03.Apply` |
| `Init` | 5,228,566 | |

Every step is under the 9,000,000 declared limit and the 10,000,000-cycle block cap. After each update the instance
head equals the rc3 run's on the same data (`lez-v03/harness/tests/equal_rc3.rs`). Refusals exit with their error
code and are charged less than their limit (`lez-v03/harness/tests/refusals.rs`).

## The planned run, simulated against the live testnet

`bv-v03 plan` with the release guests read the testnet at block 85,012 (2026-11-22, both base fees 8) and simulated,
sending nothing (`evidence/public-testnet/plan/`):

| Part | Transactions | Simulated cycles |
|---|---|---|
| Deployment through the program loader, both headers immutable | 12 | none metered (paid by their 860,132 bytes) |
| `Init` of a Sepolia instance from checkpoint slot 11,616,256 | 1 | 5,048,787 |
| Two period updates (`updates_by_range` 1418, 1419), each `Begin`, `Miller`, `Finish` | 6 | 4,443,564 to 5,786,132 per step |
| One finality update (slot 11,627,936) | 3 | 2,677,794 to 5,785,972 per step |
| One refusal (a duplicate `Finish`, refused with exit code 14) | 1 | 96,196 |

The 22 that should apply do so in simulation, and the refusal is refused as intended. Fees at the head: 390,768,528
units; with the reserve of the largest step the payer needs at least 438,448,648 units. Before each send the client
simulates again against freshly read accounts, sends only if the transaction applies, declares at most 9,000,000 gas,
sends one transaction per block and makes at most one RPC request a second.

## Funds

LEZ testnet units enter LEZ by a deposit from the Logos Blockchain testnet (channel `0101...01`), whose faucet needs
a sign-in for Logos organisation members. Our own Logos Blockchain node is syncing and will mine once it is online; a
transfer of LEZ testnet units from another holder is the other route. Paying account:
`4kST35tNxNnvQBUdLJga7EMe4DQbeX3Gjfeauw2cAwMm`.

## Re-check

```sh
cd lez-v03 && scripts/build_guests.sh local && cargo test --profile host --locked -p v03_harness
lez-v03/scripts/build_guests.sh docker        # image ids, compare with guests/IMAGE_IDS.v03.expected (built twice, equal)
```

After the run: `lez-v03/scripts/verify.sh` (against the testnet) and
`python3 -I lez-v03/scripts/verify_offline.py evidence/public-testnet` (no network).
