# The planned public-testnet run, simulated, not sent

`bv-v03 plan` (`lez-v03/client`) on 2026-11-22 at 05:05 UTC, with the release guests of `guests/IMAGE_IDS.v03.expected`
(copied here as `IMAGE_IDS`). It read the fee state and the payer's account from `https://testnet.lez.logos.co`
(LEZ testnet 0.3, `v0.3.2`; block 85,012, both base fees 8; 4 RPC requests), built and signed the 23 transactions of
the run and executed each in LEZ v0.3.2's state machine (`lee::V03State`) after the ones before it. **Nothing was
sent**: the paying account `4kST35tNxNnvQBUdLJga7EMe4DQbeX3Gjfeauw2cAwMm` held 0 units.

Data: the Sepolia checkpoint at slot 11,616,256, the period updates of `updates_by_range` 1418 and 1419, and the
finality update at slot 11,627,936, fetched on 2026-11-21 by `tools/fixtures`.

- `plan.txt`: one line per transaction: bytes, declared gas limit, simulated cycles, fee at the head's base fees.
- `plan.json`: the same, with the fee state, the largest reserve and the funds needed.

Result: 12 deployment transactions (860,132 bytes, no cycles metered), `Init` 5,048,787 cycles, nine update steps
from 2,677,794 to 5,786,132 cycles, and one refusal (a duplicate `Finish`, exit 14, 96,196 cycles). Gas limit =
simulated cycles + 3%, at most 9,000,000. Fees at the head 390,768,528 units; with the largest reserve the payer needs
at least 438,448,648 units. All figures are **simulated**; when the run is sent, the charged figures will be derived
from the payer's balance and labelled so.
