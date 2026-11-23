"""Evidence record of the chain route through CommitteeLezAnchor on Sepolia (committee_anchor_sepolia.sh record).

    python3 -I committee_anchor_record.py FLOW_DIR SEPOLIA_RPC > evidence/bridge/committee-anchor/sepolia.json

Reads the flow's saved files (no secret: neither the witness file nor the key files are read) and re-reads every
transaction receipt and the anchor's and vault's state from Sepolia. Machine paths are written as ${BUILD} and ~.
"""

import json
import os
import subprocess
import sys

flow, rpc = sys.argv[1], sys.argv[2]
BUILD = os.path.dirname(os.path.dirname(os.path.dirname(os.path.realpath(flow))))


def clean(s):
    return s.replace(BUILD, "${BUILD}").replace(os.path.expanduser("~"), "~") if isinstance(s, str) else s


def load(name):
    with open(os.path.join(flow, name)) as f:
        return json.load(f)


def env():
    out = {}
    with open(os.path.join(flow, "flow.env")) as f:
        for line in f:
            if "=" in line:
                k, v = line.rstrip("\n").split("=", 1)
                out[k] = v
    return out


def cast(*args):
    return subprocess.run(["cast", *args, "--rpc-url", rpc], check=True, capture_output=True, text=True).stdout.strip()


def receipt(tx):
    r = json.loads(cast("receipt", tx, "--json"))
    return {"tx": tx, "block": int(r["blockNumber"], 16), "gas": int(r["gasUsed"], 16), "status": int(r["status"], 16)}


e = env()
dep = load("deploy.json")
bc = load("broadcast-deploy.json")
chain = load("witness/chain.json")
exv = load("execute-vault/meta.json")
proof = load("groth16/proof/meta.json")
job = load("groth16/job/record.json")
vault, anchor = dep["vault"], dep["committee_anchor"]
with open(os.path.join(flow, "groth16/proof/journal.hex")) as f:
    journal = bytes.fromhex(f.read().strip()[2:])
names = {0: "CommitteeLezAnchor", 1: "VaultChain", 2: "registerToken(ETH)", 3: "bindLezInstance"}
deploy_txs = []
for i, r in enumerate(bc["receipts"]):
    x = receipt(r["transactionHash"])
    x["what"] = names.get(i, "?")
    if r.get("contractAddress"):
        x["contract"] = r["contractAddress"]
    deploy_txs.append(x)

refusals = [
    ("REFUSE_FEW_TX", "attestTip with 2 signatures of the 4", "TooFewSignatures()"),
    ("REFUSE_OUTSIDER_TX", "attestTip with 2 members and a fresh key outside the set ({})".format(e.get("OUTSIDER")),
     "NotASigner(address)"),
    ("REFUSE_TWICE_TX", "attestTip with one member's signature twice and one other", "SignersNotAscending()"),
    ("REFUSE_REPLAY_TX", "the tip's 3 valid signatures submitted for block {} instead of {}".format(
        int(e["TIP"]) - 1, e["TIP"]), "NotASigner(address)"),
    ("REFUSE_AGAIN_ATTEST_TX", "the same attestTip again after it was accepted", "AlreadyAttested()"),
    ("REFUSE_EARLY_TX", "releaseChain with the real seal before the 600 s delay ended", "TipNotFinal()"),
    ("REFUSE_AGAIN_TX", "the same releaseChain again", "AlreadyReleased()"),
]
release = receipt(e["RELEASE_TX"])
attest = receipt(e["ATTEST_TX"])
nullifier = "0x" + proof["nullifier"]
signers = dep["signers"]
record = {
    "what": "One real burn on seq-pub released on Sepolia through VaultChain.releaseChain, with the tip attested by "
            "CommitteeLezAnchor: 3 of 4 fixed signer keys, no owner. A Groth16 proof of guest burn_chain (unchanged, "
            "same image id) verified by the RISC Zero 3.1.0 verifier behind its emergency stop",
    "label": "measured; Sepolia (chain id 11155111); real burn on seq-pub (dev mode off); stand-in signer set: four "
             "test keys, all ours",
    "proven": "the burn transaction for this note is transaction {} of LEZ block {} (hash {}); the block does not list "
              "it as reverted; its bytes show the bridge program delivering Append{{instance, token, denom, note}} "
              "to the instance's accumulator; and blocks {}..{} are one unbroken hash chain from the pinned block {} "
              "(hash {}) to the tip {} (hash {})".format(
                  chain["tx"], chain["burn_block"], chain["burn_block_hash"], chain["pinned"], chain["tip"],
                  chain["pinned"], chain["pinned_hash"], chain["tip"], chain["tip_hash"]),
    "attested": "that block {} with hash {} is on the real LEZ chain: at least 3 of 4 named keys signed (zone, block "
                "id, block hash) for this anchor on this chain (EIP-712); the deployer posted the signatures (anyone "
                "may); the anchor accepted the tip 600 s later. No key can change the set, and the anchor has no "
                "owner".format(chain["tip"], chain["tip_hash"]),
    "not_proven": "that LEZ executed these blocks correctly (so 'not reverted' and 'sent by the bridge program' rest "
                  "on the sequencer having run and verified the transaction); that the tip is final (our standalone "
                  "sequencer reports every block as Pending, so there is no finality to read); and which chain is "
                  "the real one, now on the word of 3 of 4 keys, not 1. All four keys are ours and all four signers "
                  "read one sequencer, so today this is still one party. The keys are secp256k1 keys made for this "
                  "test; LEZ's sequencer keys are Ed25519. This does not meet RFP-022's rule of no trusted signers",
    "no_longer_trusted": "any single key, on this vault",
    "signer_note": "four keys made for this test: all four keys are ours; four signer runs on one machine reading one sequencer "
                   "(tools/bridge_cli/committee_sign.sh: each run recomputes every block hash from the served bytes "
                   "and checks each link back to the pinned block before it signs)",
    "test_arrangement": "three vaults on one LEZ instance: the same burn was released on vault v4 by the root route "
                        "(evidence/bridge/real-burn-v4/), on a second test vault by the chain route with the one-key "
                        "stub (evidence/bridge/chain-anchor/sepolia.json), and here on a third test vault. The "
                        "nullifier is the same; each vault keeps its own nullifier map and pays from its own deposit, "
                        "so the burn pays once per vault. This vault holds only the 0.001 ETH deposited for this "
                        "release. A production instance has one vault, which holds that instance's deposits",
    "lez": {
        "sequencer": "seq-pub (LEZ v0.4.0-rc3, dev mode off), read only: getBlockRange, getBlock",
        "burn_tx": "0x" + e["BURN_TXH"],
        "burn_block": chain["burn_block"], "burn_block_hash": chain["burn_block_hash"], "tx_index": chain["tx"],
        "pinned": chain["pinned"], "pinned_hash": chain["pinned_hash"],
        "tip": chain["tip"], "tip_hash": chain["tip_hash"], "note": chain["note"],
        "block_bytes": proof["block_bytes"],
    },
    "guest": {
        "name": "burn_chain", "image_id": proof["image_id"], "risc0": "3.1.0",
        "executor_this_vault": {"cycles_user": exv["cycles_user"], "segments_po2_20": exv["segments"],
                                "input_bytes": exv["input_bytes"], "journal_equals_native": exv["journal_equals_native"]},
    },
    "proof": {
        "mode": "groth16", "segment_po2": proof.get("segment_po2"), "segments": proof["segments"],
        "cycles_user": proof["cycles_user"], "prover_seconds": round(proof["seconds"], 1),
        "wall_s": job.get("wall_s"), "max_rss_kib_time_v": job.get("max_rss_kib_time_v"),
        "peak_tree_rss_kib_sampler": job.get("sampler", {}).get("peak_tree_rss_kib"),
        "cpu_pct": job.get("cpu_pct"), "load1_mean": job.get("sampler", {}).get("load1_mean"),
        "start_utc": job.get("start_utc"), "end_utc": job.get("end_utc"), "machine": job.get("machine"),
        "mem_start": job.get("mem_start"),
        "memory_note": "time -v and the sampler cover r0vm and its children. The final Groth16 compression runs in "
                       "RISC Zero's prover container (docker, outside that process tree), not measured here",
        "alone": "the only proof on the machine, in its own scope under the proving lock, started after the build "
                 "lock was free; other services kept running, so the wall time is not a quiet-machine figure",
        "journal_hex": "evidence/bridge/committee-anchor/sepolia-journal.hex",
        "seal_hex": "evidence/bridge/committee-anchor/sepolia-seal.hex",
    },
    "sepolia": {
        "verifier": dep["verifier"],
        "verifier_note": "RiscZeroVerifierEmergencyStop around RISC Zero Groth16 verifier 3.1.0 (selector 0xe71e6620)",
        "committee_anchor": anchor, "vault": vault,
        "signers": signers,
        "threshold": int(cast("call", anchor, "threshold()(uint8)")),
        "delay_s": int(cast("call", anchor, "delay()(uint64)").split()[0]),
        "zone": dep["zone"],
        "vault_tip_anchor": cast("call", vault, "tipAnchor()(address)"),
        "vault_root_anchor": cast("call", vault, "anchor()(address)"),
        "admin": dep["admin"],
        "admin_note": "the vault's admin and freezer (as on the other test vaults); it has no role in the anchor",
        "chain_image_id": dep["chain_image_id"], "pinned_id": dep["pinned_id"], "pinned_hash": dep["pinned_hash"],
        "lez_instance": cast("call", vault, "lezInstance()(bytes32)"),
        "deploy": deploy_txs,
        "deposit": receipt(e["DEPOSIT_TX"]),
        "attest_tip": dict(attest, signatures=3, note="sent by the deployer, which is not a signer"),
        "refusals": [dict(receipt(e[k]), what=w, expected=x) for k, w, x in refusals],
        "refusals_note": "each refusal was first simulated with cast call and its error selector matched, then sent "
                         "with a fixed gas limit of 300,000 so that it is mined; a receipt does not carry the revert "
                         "reason (the flow logs keep the simulated reasons)",
        "release_chain": release,
        "nullifier": nullifier,
        "nullifier_spent": cast("call", vault, "burnNullifierSpent(bytes32)(bool)", nullifier),
        "vault_balance_wei_after": cast("balance", vault),
        "recipient": "0x" + journal[11 * 32 + 12:12 * 32].hex(),
        "fee_to_relayer_wei": int.from_bytes(journal[14 * 32:15 * 32], "big"),
    },
}
print(json.dumps(json.loads(clean(json.dumps(record))), indent=1))
