"""Evidence record of the chain route on Sepolia (tools/bridge_cli/chain_anchor_sepolia.sh record).

    python3 -I chain_anchor_record.py FLOW_DIR SEPOLIA_RPC > evidence/bridge/chain-anchor/sepolia.json

Reads the flow's saved files (no secret: the witness file is not read) and re-reads every transaction receipt and the
vault's state from Sepolia. Machine paths are written as ${BUILD} (the build folder) and ~.
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
ex = load("execute-v4vault/meta.json")
exv = load("execute-vault/meta.json")
proof = load("groth16/proof/meta.json")
job = load("groth16/job/record.json")
vault = dep["vault"]
with open(os.path.join(flow, "groth16/proof/journal.hex")) as f:
    journal = bytes.fromhex(f.read().strip()[2:])
names = {0: "StubLezAnchor (root route, unused here)", 1: "StubLezTipAnchor", 2: "VaultChain", 3: "registerToken(ETH)",
         4: "bindLezInstance"}
deploy_txs = []
for i, r in enumerate(bc["receipts"]):
    x = receipt(r["transactionHash"])
    x["what"] = names.get(i, "?")
    if r.get("contractAddress"):
        x["contract"] = r["contractAddress"]
    deploy_txs.append(x)
release = receipt(e["RELEASE_TX"])
nullifier = "0x" + proof["nullifier"]
record = {
    "what": "One real burn on seq-pub released on Sepolia through VaultChain.releaseChain: a Groth16 proof of guest "
            "burn_chain verified by the RISC Zero 3.1.0 verifier behind its emergency stop, and one tip attested by "
            "the STUB tip anchor",
    "label": "measured; Sepolia (chain id 11155111); real burn on seq-pub (dev mode off); STUB tip anchor (our key)",
    "proven": "the burn transaction for this note is transaction {} of LEZ block {} (hash {}); the block does not list "
              "it as reverted; its bytes show the bridge program delivering Append{{instance, token, denom, note}} "
              "to the instance's accumulator; and blocks {}..{} are one unbroken hash chain from the pinned block {} "
              "(hash {}) to the tip {} (hash {})".format(
                  chain["tx"], chain["burn_block"], chain["burn_block_hash"], chain["pinned"], chain["tip"],
                  chain["pinned"], chain["pinned_hash"], chain["tip"], chain["tip_hash"]),
    "attested": "that block {} with hash {} is on the real, final LEZ chain: attested by the STUB tip anchor's owner "
                "key (ours), accepted after 600 s".format(chain["tip"], chain["tip_hash"]),
    "not_proven": "that LEZ executed these blocks correctly (so 'not reverted' and 'sent by the bridge program' rest "
                  "on the sequencer having run and verified the transaction), finality, and which chain is the real "
                  "one (anyone can build a hash chain from the pinned block)",
    "test_arrangement": "two vaults on one LEZ instance: the same burn was released on vault v4 by the root route "
                        "(evidence/bridge/real-burn-v4/) and here on a test vault by the chain route. The nullifier "
                        "is the same; each vault keeps its own nullifier map, so the burn pays once per vault. The "
                        "test vault holds only the 0.001 ETH deposited for this release. A production instance has "
                        "one vault, which holds that instance's deposits",
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
        "executor": {"cycles_user": ex["cycles_user"], "segments_po2_20": ex["segments"], "input_bytes": ex["input_bytes"],
                     "journal_equals_native": ex["journal_equals_native"]},
        "executor_test_vault": {"cycles_user": exv["cycles_user"], "segments_po2_20": exv["segments"]},
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
                       "RISC Zero's prover container (docker, outside that process tree); it was seen at 5,043,400 KiB "
                       "RSS in one ps sample (a lower bound for its peak, not measured as a peak)",
        "alone": "the only proof on the machine, in its own scope under the proving lock, started after the build "
                 "lock was free; other services kept running (the sequencers and a Logos node, load1 mean above), so "
                 "the wall time is not a quiet-machine figure",
        "journal_hex": "evidence/bridge/chain-anchor/sepolia-journal.hex",
        "seal_hex": "evidence/bridge/chain-anchor/sepolia-seal.hex",
        "foundry": "contracts/test/VaultChainSepolia.t.sol checks this seal and journal with the 3.1.0 verifier's "
                   "Sepolia runtime bytecode",
    },
    "sepolia": {
        "verifier": dep["verifier"],
        "verifier_note": "RiscZeroVerifierEmergencyStop around RISC Zero Groth16 verifier 3.1.0 (selector 0xe71e6620)",
        "vault": vault, "tip_anchor_stub": dep["tip_anchor_stub"], "anchor_stub": dep["anchor_stub"],
        "admin": dep["admin"], "chain_image_id": dep["chain_image_id"],
        "pinned_id": dep["pinned_id"], "pinned_hash": dep["pinned_hash"],
        "lez_instance": cast("call", vault, "lezInstance()(bytes32)"),
        "deploy": deploy_txs,
        "deposit": receipt(e["DEPOSIT_TX"]),
        "attest_tip": receipt(e["ATTEST_TX"]),
        "refused_before_delay": dict(receipt(e["REFUSE_EARLY_TX"]), expected="TipNotFinal()")
        if "REFUSE_EARLY_TX" in e else None,
        "release_chain": release,
        "refused_repeat": dict(receipt(e["REFUSE_AGAIN_TX"]), expected="AlreadyReleased()")
        if "REFUSE_AGAIN_TX" in e else None,
        "nullifier": nullifier,
        "nullifier_spent": cast("call", vault, "burnNullifierSpent(bytes32)(bool)", nullifier),
        "vault_balance_wei_after": cast("balance", vault),
        "recipient": "0x" + journal[11 * 32 + 12:12 * 32].hex(),
        "fee_to_relayer_wei": int.from_bytes(journal[14 * 32:15 * 32], "big"),
    },
}
print(json.dumps(json.loads(clean(json.dumps(record))), indent=1))
