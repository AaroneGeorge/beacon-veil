// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {Test} from "forge-std/Test.sol";
import {VerificationFailed} from "risc0/IRiscZeroVerifier.sol";
import {IRiscZeroVerifier} from "risc0/IRiscZeroVerifier.sol";
import {Vault} from "../src/Vault.sol";
import {VaultChain} from "../src/VaultChain.sol";
import {StubLezAnchor} from "../src/StubLezAnchor.sol";
import {StubLezTipAnchor} from "../src/StubLezTipAnchor.sol";

/// The real Groth16 seal and journal of guest `burn_chain` for the burn in `seq-pub` block 6588, as released on
/// Sepolia (evidence/bridge/chain-anchor/sepolia.json, sepolia-{seal,journal}.hex). The verifier is the RISC Zero
/// Groth16 verifier 3.1.0 itself: its runtime bytecode as deployed on Sepolia at
/// 0x8c915b5071206C6E328BC18529586c513408aB3b (`cast code` at block 12186895, test/vectors/
/// risc0-groth16-verifier-3.1.0-sepolia.hex, sha256 d898dd5a...0f24 of the file), placed at a test address. The
/// pinned risc0-ethereum library (v3.0.1) has another verifying key, so it cannot check a 3.1.0 seal.
contract VaultChainSepoliaTest is Test {
    bytes4 constant SELECTOR_3_1_0 = 0xe71e6620;
    /// guest burn_chain, reproducible docker build (evidence/bridge/chain-anchor/guest.json).
    bytes32 constant CHAIN_IMAGE_ID = 0x88c00a4e43fc6b8b7b776e7a540422d1c684251751940901ce3256eecbd26dd3;

    bytes constant SEAL =
        hex"e71e662009796ffff3ad019035a4c8e94e3c4c1837e5a347f48f3090a9d47b106cb1930600f5f61a6e6b4dc6af160fd1c1cbf95400ed226dd864deda7d7f83f42f57136b0082b9fba48ed78c11eec85c8c43f5b28d7ffddb9cbfffc74e96d8d0322ddc761df3e16e09a583f4a125df08f6ff18f5ee0b7c7518b80706f30d504cdc5a3a6221a38553f1a583e5665fdac8a1ec8a0545bc7056deea000fdf9c8e5290e719d02cff3b2cde71fb218b4eb30d80c3315b83b6c536dc9bed7ec34b422b4d39787a14f39334c040172a540157ab43a78380901bea17e33337ba113fc2974d77c2452e2799746798648d744750b853a21efb063af819c1628f38eacdb68464606bf8";
    bytes constant JOURNAL =
        hex"00000000000000000000000000000000000000000000000000000000000000020000000000000000000000000000000000000000000000000000000000aa36a70000000000000000000000007aeb1ee888b618466b0313f7f7ea019213a16a572a4029eb933413538fa666548f8936eeb4b75a70ba0bab2901900c2fdcc428e0b9c51c4bd7493530202e80cdff38dbcb3cee9fb16ee4a694a4e6ff5adbc4a8c800000000000000000000000000000000000000000000000000000000000019b7d9c5232af729a98d1715a55fb617889b3b83fdbf977784f6b7cf42d628763d3300000000000000000000000000000000000000000000000000000000000019c1bf864fcacea33edb5c794bcd98353952cef4ac5696b085fd81b4224ef6cb247c00000000000000000000000000000000000000000000000000000000000019bc459558bea6bf8c11763d8f6d01edc5c8a99edd6b19a6f31152ca720018e7c711000000000000000000000000523072a2ac5e5a3e51c107bc17995a3fa03dbd78000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000038d7ea4c68000000000000000000000000000000000000000000000000000000009184e72a000";

    address admin = address(0xA11CE);
    address relayer = address(0x5E1A7);
    IRiscZeroVerifier verifier;
    StubLezTipAnchor tipAnchor;
    VaultChain vault;
    VaultChain.ChainJournal j;

    function setUp() public {
        verifier = IRiscZeroVerifier(address(0x8c915b5071206C6E328BC18529586c513408aB3b));
        vm.etch(
            address(verifier),
            vm.parseBytes(vm.trim(vm.readFile("test/vectors/risc0-groth16-verifier-3.1.0-sepolia.hex")))
        );
        j = abi.decode(JOURNAL, (VaultChain.ChainJournal));
        vm.chainId(j.chainId);
        tipAnchor = new StubLezTipAnchor(admin, 600);
        StubLezAnchor rootAnchor = new StubLezAnchor(admin, 600);
        // The vault at the journal's address, pinned at the journal's block, as deployed on Sepolia.
        deployCodeTo(
            "VaultChain.sol:VaultChain",
            abi.encode(
                verifier,
                rootAnchor,
                bytes32(uint256(1)),
                keccak256("lez-rc3-standalone"),
                j.lezProgram,
                admin,
                1 days,
                VaultChain.ChainRoute(tipAnchor, CHAIN_IMAGE_ID, uint64(j.pinnedId), j.pinnedHash)
            ),
            j.vault
        );
        vault = VaultChain(payable(j.vault));
        uint256[] memory d = new uint256[](1);
        d[0] = 0.001 ether;
        vm.startPrank(admin);
        vault.registerToken(vault.NATIVE(), d, 1 ether, 1 ether);
        vault.bindLezInstance(j.lezInstance);
        vm.stopPrank();
        vm.deal(address(this), 1 ether);
        vault.deposit{value: 0.001 ether}(vault.NATIVE(), 0, keccak256("funding"));
    }

    function test_journalIsTheSepoliaRelease() public view {
        assertEq(j.version, 2);
        assertEq(j.chainId, 11155111);
        assertEq(j.pinnedId, 6583);
        assertEq(j.tipId, 6593);
        assertEq(j.burnBlockId, 6588);
        assertEq(j.denom, 0.001 ether);
        assertEq(j.fee, 0.00001 ether);
        assertEq(Selector(address(verifier)).SELECTOR(), SELECTOR_3_1_0);
        assertEq(Selector(address(verifier)).VERSION(), "3.1.0");
    }

    function test_realSealVerifies() public view {
        verifier.verify(SEAL, CHAIN_IMAGE_ID, sha256(JOURNAL));
    }

    function test_realSealRefusedForAnotherJournal() public {
        bytes memory other = JOURNAL;
        other[479] = bytes1(uint8(other[479]) ^ 1); // the fee's last byte
        bytes32 digest = sha256(other); // before expectRevert: sha256 is a precompile call
        vm.expectRevert(VerificationFailed.selector);
        verifier.verify(SEAL, CHAIN_IMAGE_ID, digest);
    }

    function test_realSealReleasesOnceAfterTheTipIsFinal() public {
        vm.prank(relayer);
        vm.expectRevert(VaultChain.TipNotFinal.selector);
        vault.releaseChain(SEAL, JOURNAL);

        vm.prank(admin);
        tipAnchor.attestTip(uint64(j.tipId), j.tipHash);
        vm.warp(vm.getBlockTimestamp() + 600);
        uint256 r0 = j.recipient.balance;
        vm.prank(relayer);
        vault.releaseChain(SEAL, JOURNAL);
        assertEq(j.recipient.balance - r0, j.denom - j.fee);
        assertEq(relayer.balance, j.fee);
        assertTrue(vault.burnNullifierSpent(j.nullifier));

        vm.prank(relayer);
        vm.expectRevert(Vault.AlreadyReleased.selector);
        vault.releaseChain(SEAL, JOURNAL);
    }

    function test_realSealRefusedOnAnotherTip() public {
        vm.prank(admin);
        tipAnchor.attestTip(uint64(j.tipId), keccak256("another chain"));
        vm.warp(vm.getBlockTimestamp() + 600);
        vm.prank(relayer);
        vm.expectRevert(VaultChain.TipNotFinal.selector);
        vault.releaseChain(SEAL, JOURNAL);
    }
}

interface Selector {
    function SELECTOR() external view returns (bytes4);
    function VERSION() external view returns (string memory);
}
