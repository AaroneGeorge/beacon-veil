// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {Test} from "forge-std/Test.sol";
import {IRiscZeroVerifier} from "risc0/IRiscZeroVerifier.sol";
import {Vault} from "../src/Vault.sol";

contract VaultSkeletonTest is Test {
    function test_constructorStoresVerifier() public {
        IRiscZeroVerifier v = IRiscZeroVerifier(address(0xBEEF));
        Vault vault = new Vault(v);
        assertEq(address(vault.verifier()), address(v));
    }
}
