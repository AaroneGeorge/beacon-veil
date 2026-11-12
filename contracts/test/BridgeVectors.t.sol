// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {Test} from "forge-std/Test.sol";
import {RiscZeroMockVerifier} from "risc0/test/RiscZeroMockVerifier.sol";
import {Vault} from "../src/Vault.sol";
import {StubLezAnchor} from "../src/StubLezAnchor.sol";

/// @notice Rust (programs/bridge) and Solidity must agree byte for byte. Vectors: `bridge_cli vectors --out
/// contracts/test/vectors/bridge.json`.
contract BridgeVectorsTest is Test {
    string json;
    Vault vault;
    address token;
    uint256 denom;

    function setUp() public {
        json = vm.readFile("test/vectors/bridge.json");
        token = vm.parseJsonAddress(json, ".token");
        denom = vm.parseUint(vm.parseJsonString(json, ".denom"));
        RiscZeroMockVerifier verifier = new RiscZeroMockVerifier(bytes4(0));
        StubLezAnchor anchor = new StubLezAnchor(address(this), 0);
        vault = new Vault(verifier, anchor, bytes32(0), bytes32(0), bytes32(0), address(this), 1 days);
    }

    function test_leafHashMatchesRust() public view {
        bytes32 c = vm.parseJsonBytes32(json, ".commitment");
        assertEq(vault.leafHash(token, denom, c), vm.parseJsonBytes32(json, ".leaf"));
    }

    function test_treeRootsMatchRust() public {
        bytes32[] memory commitments = vm.parseJsonBytes32Array(json, ".leaves_commitments");
        bytes32[] memory roots = vm.parseJsonBytes32Array(json, ".roots");
        // Deposits of a stand-in token placed at the vector's token address.
        vm.etch(token, address(new MockPull()).code);
        uint256[] memory d = new uint256[](1);
        d[0] = denom;
        vault.registerToken(token, d, type(uint256).max, type(uint256).max);
        for (uint256 i = 0; i < commitments.length; i++) {
            vault.deposit(token, 0, commitments[i]);
            assertEq(vault.depositRoot(), roots[i]);
        }
    }

    function test_journalDecodesAsRustEncoded() public view {
        bytes memory j = vm.parseJsonBytes(json, ".journal");
        assertEq(j.length, vault.JOURNAL_LENGTH());
        Vault.BurnJournal memory d = abi.decode(j, (Vault.BurnJournal));
        assertEq(d.version, 1);
        assertEq(d.chainId, 31337);
        assertEq(d.vault, address(0x3333333333333333333333333333333333333333));
        assertEq(d.lezProgram, bytes32(0x4444444444444444444444444444444444444444444444444444444444444444));
        assertEq(d.lezInstance, bytes32(0x5555555555555555555555555555555555555555555555555555555555555555));
        assertEq(d.accumulatorRoot, vm.parseJsonBytes32(json, ".journal_root"));
        assertEq(d.nullifier, vm.parseJsonBytes32(json, ".journal_nullifier"));
        assertEq(d.recipient, address(0x2222222222222222222222222222222222222222));
        assertEq(d.token, token);
        assertEq(d.denom, denom);
        assertEq(d.fee, 5);
    }

    function test_rootSlotConstantMatchesRust() public view {
        assertEq(vault.DEPOSIT_ROOT_SLOT(), 0xc48ce8c9c69c089f8e403bb82c6c6a3b0b082f197e29f63d5843b5b37b73a200);
    }
}

/// @notice Token stand-in whose transferFrom moves a balance the vault can observe.
contract MockPull {
    mapping(address => uint256) public balanceOf;

    function transferFrom(address, address to, uint256 amount) external returns (bool) {
        balanceOf[to] += amount;
        return true;
    }
}
