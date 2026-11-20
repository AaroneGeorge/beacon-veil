// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {Script, console2} from "forge-std/Script.sol";
import {IRiscZeroVerifier} from "risc0/IRiscZeroVerifier.sol";
import {RiscZeroMockVerifier} from "risc0/test/RiscZeroMockVerifier.sol";
import {VaultChain} from "../src/VaultChain.sol";
import {StubLezAnchor} from "../src/StubLezAnchor.sol";
import {StubLezTipAnchor} from "../src/StubLezTipAnchor.sol";

/// @notice Deploys StubLezAnchor (STUB, for the inherited root route), StubLezTipAnchor (STUB) and VaultChain, registers
/// ETH and binds the LEZ instance.
///
/// Env (all read here, none printed):
///   PRIVATE_KEY      deployer key (admin, freezer, owner of both stubs)
///   VERIFIER         IRiscZeroVerifier address; unset or 0 = deploy RiscZeroMockVerifier(0x00000000) (anvil only)
///   BURN_IMAGE_ID    image id of guest burn_proof (root route)
///   CHAIN_IMAGE_ID   image id of guest burn_chain
///   LEZ_ZONE         opaque id of the LEZ chain + zone, default keccak("lez-rc3-standalone")
///   LEZ_PROGRAM      account id of program bridge
///   LEZ_INSTANCE     bridge instance id, bound at once
///   PINNED_ID        the pinned LEZ block id
///   PINNED_HASH      its hash
///   ANCHOR_DELAY     seconds for both stubs (default 600)
///   CAP_WINDOW       seconds (default 86400)
///   ETH_DENOMS       comma list in wei (default 0.001 ether, 0.01 ether)
contract DeployChain is Script {
    function run() external {
        uint256 pk = vm.envUint("PRIVATE_KEY");
        address admin = vm.addr(pk);
        address v = vm.envOr("VERIFIER", address(0));
        uint64 delay = uint64(vm.envOr("ANCHOR_DELAY", uint256(600)));
        uint256[] memory d = new uint256[](2);
        d[0] = 0.001 ether;
        d[1] = 0.01 ether;
        uint256[] memory ethDenoms = vm.envOr("ETH_DENOMS", ",", d);
        VaultChain.ChainRoute memory route;
        route.chainImageId = vm.envBytes32("CHAIN_IMAGE_ID");
        route.pinnedId = uint64(vm.envUint("PINNED_ID"));
        route.pinnedHash = vm.envBytes32("PINNED_HASH");

        vm.startBroadcast(pk);
        IRiscZeroVerifier verifier;
        if (v == address(0)) {
            require(block.chainid == 31337, "mock verifier only on anvil");
            verifier = new RiscZeroMockVerifier(bytes4(0));
        } else {
            verifier = IRiscZeroVerifier(v);
        }
        StubLezAnchor anchor = new StubLezAnchor(admin, delay);
        route.tipAnchor = new StubLezTipAnchor(admin, delay);
        VaultChain vault = new VaultChain(
            verifier,
            anchor,
            vm.envBytes32("BURN_IMAGE_ID"),
            vm.envOr("LEZ_ZONE", keccak256("lez-rc3-standalone")),
            vm.envBytes32("LEZ_PROGRAM"),
            admin,
            vm.envOr("CAP_WINDOW", uint256(1 days)),
            route
        );
        vault.registerToken(vault.NATIVE(), ethDenoms, 1 ether, 1 ether);
        vault.bindLezInstance(vm.envBytes32("LEZ_INSTANCE"));
        vm.stopBroadcast();

        string memory obj = "deploy";
        vm.serializeUint(obj, "chain_id", block.chainid);
        vm.serializeAddress(obj, "verifier", address(verifier));
        vm.serializeAddress(obj, "anchor_stub", address(anchor));
        vm.serializeAddress(obj, "tip_anchor_stub", address(route.tipAnchor));
        vm.serializeAddress(obj, "admin", admin);
        vm.serializeBytes32(obj, "chain_image_id", route.chainImageId);
        vm.serializeUint(obj, "pinned_id", route.pinnedId);
        vm.serializeBytes32(obj, "pinned_hash", route.pinnedHash);
        vm.serializeUint(obj, "block", block.number);
        string memory json = vm.serializeAddress(obj, "vault", address(vault));
        console2.log(json);
    }
}

/// @notice STUB anchor owner attests one LEZ block. Env: PRIVATE_KEY, TIP_ANCHOR, TIP_ID, TIP_HASH.
contract AttestTip is Script {
    function run() external {
        uint256 pk = vm.envUint("PRIVATE_KEY");
        StubLezTipAnchor a = StubLezTipAnchor(vm.envAddress("TIP_ANCHOR"));
        vm.startBroadcast(pk);
        a.attestTip(uint64(vm.envUint("TIP_ID")), vm.envBytes32("TIP_HASH"));
        vm.stopBroadcast();
    }
}

/// @notice One release on the chain route. Env: PRIVATE_KEY (any relayer), VAULT, SEAL, JOURNAL.
contract ReleaseChainOne is Script {
    function run() external {
        uint256 pk = vm.envUint("PRIVATE_KEY");
        VaultChain vault = VaultChain(payable(vm.envAddress("VAULT")));
        bytes memory seal = vm.envBytes("SEAL");
        bytes memory journal = vm.envBytes("JOURNAL");
        vm.startBroadcast(pk);
        vault.releaseChain(seal, journal);
        vm.stopBroadcast();
    }
}
