// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {Script, console2} from "forge-std/Script.sol";
import {IRiscZeroVerifier} from "risc0/IRiscZeroVerifier.sol";
import {RiscZeroMockVerifier} from "risc0/test/RiscZeroMockVerifier.sol";
import {Vault} from "../src/Vault.sol";
import {StubLezAnchor} from "../src/StubLezAnchor.sol";
import {MockUsdt} from "../test/mocks/Tokens.sol";

/// @notice Deploys StubLezAnchor (STUB), a USDT-style test token and the Vault, and registers ETH and the token.
///
/// Env (all read here, none printed):
///   PRIVATE_KEY      deployer key (admin, freezer, anchor owner)
///   VERIFIER         IRiscZeroVerifier address; unset or 0 = deploy RiscZeroMockVerifier(0x00000000) (anvil only)
///   BURN_IMAGE_ID    image id of guest burn_proof (bytes32)
///   LEZ_ZONE         opaque id of the LEZ chain + zone (bytes32), default keccak("lez-rc3-standalone")
///   LEZ_PROGRAM      account id of program bridge (bytes32)
///   ANCHOR_DELAY     seconds (default 600)
///   CAP_WINDOW       seconds (default 86400)
///   ETH_DENOMS       comma list in wei (default 0.001 ether, 0.01 ether)
/// Sepolia: VERIFIER = RiscZeroGroth16Verifier 3.0.0, 0x2a098988600d87650Fb061FfAff08B97149Fa84D (immutable, not the
/// owner-upgradable router: Security #8). Source: lib/risc0-ethereum/contracts/deployment.toml.
contract Deploy is Script {
    struct Cfg {
        uint256 pk;
        address admin;
        address verifier;
        bytes32 imageId;
        bytes32 lezZone;
        bytes32 lezProgram;
        uint64 anchorDelay;
        uint256 capWindow;
        uint256[] ethDenoms;
    }

    function _cfg() internal view returns (Cfg memory c) {
        c.pk = vm.envUint("PRIVATE_KEY");
        c.admin = vm.addr(c.pk);
        c.verifier = vm.envOr("VERIFIER", address(0));
        c.imageId = vm.envBytes32("BURN_IMAGE_ID");
        c.lezZone = vm.envOr("LEZ_ZONE", keccak256("lez-rc3-standalone"));
        c.lezProgram = vm.envBytes32("LEZ_PROGRAM");
        c.anchorDelay = uint64(vm.envOr("ANCHOR_DELAY", uint256(600)));
        c.capWindow = vm.envOr("CAP_WINDOW", uint256(1 days));
        uint256[] memory d = new uint256[](2);
        d[0] = 0.001 ether;
        d[1] = 0.01 ether;
        c.ethDenoms = vm.envOr("ETH_DENOMS", ",", d);
    }

    function run() external {
        Cfg memory c = _cfg();
        vm.startBroadcast(c.pk);
        IRiscZeroVerifier verifier;
        if (c.verifier == address(0)) {
            require(block.chainid == 31337, "mock verifier only on anvil");
            verifier = new RiscZeroMockVerifier(bytes4(0));
        } else {
            verifier = IRiscZeroVerifier(c.verifier);
        }
        StubLezAnchor anchor = new StubLezAnchor(c.admin, c.anchorDelay);
        Vault vault = new Vault(verifier, anchor, c.imageId, c.lezZone, c.lezProgram, c.admin, c.capWindow);
        MockUsdt usdt = new MockUsdt();
        vault.registerToken(vault.NATIVE(), c.ethDenoms, 1 ether, 1 ether);
        uint256[] memory usdtDenoms = new uint256[](2);
        usdtDenoms[0] = 100e6;
        usdtDenoms[1] = 1000e6;
        vault.registerToken(address(usdt), usdtDenoms, 1_000_000e6, 1_000_000e6);
        usdt.mint(c.admin, 100_000e6);
        vm.stopBroadcast();
        _report(c, address(verifier), address(anchor), address(vault), address(usdt));
    }

    function _report(Cfg memory c, address verifier, address anchor, address vault, address usdt) internal {
        string memory obj = "deploy";
        vm.serializeUint(obj, "chain_id", block.chainid);
        vm.serializeAddress(obj, "verifier", verifier);
        vm.serializeAddress(obj, "anchor_stub", anchor);
        vm.serializeAddress(obj, "test_usdt", usdt);
        vm.serializeAddress(obj, "admin", c.admin);
        vm.serializeBytes32(obj, "burn_image_id", c.imageId);
        vm.serializeBytes32(obj, "lez_zone", c.lezZone);
        vm.serializeBytes32(obj, "lez_program", c.lezProgram);
        vm.serializeUint(obj, "block", block.number);
        string memory json = vm.serializeAddress(obj, "vault", vault);
        console2.log(json);
    }
}

/// @notice One deposit. Env: PRIVATE_KEY, VAULT, TOKEN (0 = ETH), DENOM_INDEX, COMMITMENT (bytes32).
/// The commitment comes from `bridge_cli deposit-commitment`; this script never sees the destination.
contract DepositOne is Script {
    function run() external {
        uint256 pk = vm.envUint("PRIVATE_KEY");
        Vault vault = Vault(payable(vm.envAddress("VAULT")));
        address token = vm.envOr("TOKEN", address(0));
        uint256 denomIndex = vm.envUint("DENOM_INDEX");
        bytes32 commitment = vm.envBytes32("COMMITMENT");
        (,,,, uint256[] memory denoms,,) = vault.tokenConfig(token);
        uint256 denom = denoms[denomIndex];

        vm.startBroadcast(pk);
        if (token == address(0)) {
            vault.deposit{value: denom}(token, denomIndex, commitment);
        } else {
            // USDT-style tokens refuse a non-zero to non-zero allowance change: approve exactly, from zero.
            (bool ok,) = token.call(abi.encodeWithSignature("approve(address,uint256)", address(vault), 0));
            require(ok, "approve 0");
            (ok,) = token.call(abi.encodeWithSignature("approve(address,uint256)", address(vault), denom));
            require(ok, "approve");
            vault.deposit(token, denomIndex, commitment);
        }
        vm.stopBroadcast();
        console2.log("deposit root", vm.toString(vault.depositRoot()));
        console2.log("deposit count", vault.depositCount());
    }
}

/// @notice Binds the LEZ bridge instance (once). Env: PRIVATE_KEY, VAULT, LEZ_INSTANCE.
contract BindInstance is Script {
    function run() external {
        uint256 pk = vm.envUint("PRIVATE_KEY");
        Vault vault = Vault(payable(vm.envAddress("VAULT")));
        vm.startBroadcast(pk);
        vault.bindLezInstance(vm.envBytes32("LEZ_INSTANCE"));
        vm.stopBroadcast();
    }
}

/// @notice STUB anchor: the owner posts a LEZ burn-accumulator root. Env: PRIVATE_KEY, ANCHOR, LEZ_INSTANCE, ROOT.
contract PostRoot is Script {
    function run() external {
        uint256 pk = vm.envUint("PRIVATE_KEY");
        StubLezAnchor anchor = StubLezAnchor(vm.envAddress("ANCHOR"));
        vm.startBroadcast(pk);
        anchor.post(vm.envBytes32("LEZ_INSTANCE"), vm.envBytes32("ROOT"));
        vm.stopBroadcast();
    }
}

/// @notice Release. Env: PRIVATE_KEY (relayer, receives the fee), VAULT, SEAL and JOURNAL (0x hex, as written by
/// `bridge_cli prove` into seal.hex and journal.hex).
contract ReleaseOne is Script {
    function run() external {
        uint256 pk = vm.envUint("PRIVATE_KEY");
        Vault vault = Vault(payable(vm.envAddress("VAULT")));
        bytes memory seal = vm.envBytes("SEAL");
        bytes memory journal = vm.envBytes("JOURNAL");
        vm.startBroadcast(pk);
        vault.release(seal, journal);
        vm.stopBroadcast();
    }
}

/// @notice Freezer: stop deposits and releases on a vault (global flags). Env: PRIVATE_KEY, VAULT.
/// Used 2026-11-12 on vaults v1 and v2 after RISC Zero GHSA-g6qg-9j8p-7j87 (their verifier 3.0.0 is affected).
contract FreezeAll is Script {
    function run() external {
        uint256 pk = vm.envUint("PRIVATE_KEY");
        Vault vault = Vault(payable(vm.envAddress("VAULT")));
        vm.startBroadcast(pk);
        vault.setGlobalFreeze(true, true);
        vm.stopBroadcast();
    }
}
