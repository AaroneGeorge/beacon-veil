// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {Script, console2} from "forge-std/Script.sol";
import {IRiscZeroVerifier} from "risc0/IRiscZeroVerifier.sol";
import {RiscZeroMockVerifier} from "risc0/test/RiscZeroMockVerifier.sol";
import {VaultChain} from "../src/VaultChain.sol";
import {CommitteeLezAnchor} from "../src/CommitteeLezAnchor.sol";

/// @notice Deploys one CommitteeLezAnchor (SPIKE, stand-in key set) and a VaultChain that uses it for both the root
/// route and the chain route, registers ETH and binds the LEZ instance. Same as DeployChain.s.sol otherwise.
///
/// Env (all read here, none printed):
///   PRIVATE_KEY      deployer key (vault admin and freezer; it has no role in the anchor)
///   SIGNERS          comma list of signer addresses, strictly ascending
///   THRESHOLD        signatures needed (at least LEZ's rule for the set size, at most the size)
///   VERIFIER         IRiscZeroVerifier address; unset or 0 = deploy RiscZeroMockVerifier(0x00000000) (anvil only)
///   BURN_IMAGE_ID    image id of guest burn_proof (root route)
///   CHAIN_IMAGE_ID   image id of guest burn_chain
///   LEZ_ZONE         opaque id of the LEZ chain + zone, default keccak("lez-rc3-standalone"); also the anchor's zone
///   LEZ_PROGRAM      account id of program bridge
///   LEZ_INSTANCE     bridge instance id, bound at once
///   PINNED_ID        the pinned LEZ block id
///   PINNED_HASH      its hash
///   ANCHOR_DELAY     seconds (default 600)
///   CAP_WINDOW       seconds (default 86400)
///   ETH_DENOMS       comma list in wei (default 0.001 ether, 0.01 ether)
contract DeployCommittee is Script {
    function run() external {
        uint256 pk = vm.envUint("PRIVATE_KEY");
        address v = vm.envOr("VERIFIER", address(0));
        bytes32 zone = vm.envOr("LEZ_ZONE", keccak256("lez-rc3-standalone"));

        vm.startBroadcast(pk);
        IRiscZeroVerifier verifier;
        if (v == address(0)) {
            require(block.chainid == 31337, "mock verifier only on anvil");
            verifier = new RiscZeroMockVerifier(bytes4(0));
        } else {
            verifier = IRiscZeroVerifier(v);
        }
        CommitteeLezAnchor anchor = new CommitteeLezAnchor(
            vm.envAddress("SIGNERS", ","),
            uint8(vm.envUint("THRESHOLD")),
            uint64(vm.envOr("ANCHOR_DELAY", uint256(600))),
            zone
        );
        VaultChain vault = _vault(verifier, anchor, zone, vm.addr(pk));
        vm.stopBroadcast();

        _print(verifier, anchor, vault, vm.addr(pk));
    }

    function _vault(IRiscZeroVerifier verifier, CommitteeLezAnchor anchor, bytes32 zone, address admin)
        internal
        returns (VaultChain vault)
    {
        VaultChain.ChainRoute memory route = VaultChain.ChainRoute(
            anchor, vm.envBytes32("CHAIN_IMAGE_ID"), uint64(vm.envUint("PINNED_ID")), vm.envBytes32("PINNED_HASH")
        );
        vault = new VaultChain(
            verifier,
            anchor,
            vm.envBytes32("BURN_IMAGE_ID"),
            zone,
            vm.envBytes32("LEZ_PROGRAM"),
            admin,
            vm.envOr("CAP_WINDOW", uint256(1 days)),
            route
        );
        uint256[] memory d = new uint256[](2);
        d[0] = 0.001 ether;
        d[1] = 0.01 ether;
        vault.registerToken(vault.NATIVE(), vm.envOr("ETH_DENOMS", ",", d), 1 ether, 1 ether);
        vault.bindLezInstance(vm.envBytes32("LEZ_INSTANCE"));
    }

    function _print(IRiscZeroVerifier verifier, CommitteeLezAnchor anchor, VaultChain vault, address admin) internal {
        string memory obj = "deploy";
        vm.serializeUint(obj, "chain_id", block.chainid);
        vm.serializeAddress(obj, "verifier", address(verifier));
        vm.serializeAddress(obj, "committee_anchor", address(anchor));
        vm.serializeAddress(obj, "signers", anchor.signers());
        vm.serializeUint(obj, "threshold", anchor.threshold());
        vm.serializeUint(obj, "delay", anchor.delay());
        vm.serializeBytes32(obj, "zone", anchor.zone());
        vm.serializeAddress(obj, "admin", admin);
        vm.serializeBytes32(obj, "chain_image_id", vault.chainImageId());
        vm.serializeUint(obj, "pinned_id", vault.pinnedId());
        vm.serializeBytes32(obj, "pinned_hash", vault.pinnedHash());
        vm.serializeUint(obj, "block", block.number);
        string memory json = vm.serializeAddress(obj, "vault", address(vault));
        console2.log(json);
    }
}
