// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {Test} from "forge-std/Test.sol";
import {IRiscZeroVerifier, VerificationFailed} from "risc0/IRiscZeroVerifier.sol";
import {RiscZeroMockVerifier} from "risc0/test/RiscZeroMockVerifier.sol";
import {IAccessControl} from "@openzeppelin/contracts/access/IAccessControl.sol";
import {SafeERC20} from "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
import {Vault} from "../src/Vault.sol";
import {StubLezAnchor} from "../src/StubLezAnchor.sol";
import {MockToken, MockUsdt, MockFeeToken, RejectEth} from "./mocks/Tokens.sol";

contract VaultTest is Test {
    Vault vault;
    StubLezAnchor anchor;
    RiscZeroMockVerifier verifier;
    MockToken usdc;
    MockUsdt usdt;
    MockFeeToken feeToken;

    address admin = address(0xA11CE);
    address user = address(0xB0B);
    address relayer = address(0x5E1A7);
    address payable recipient = payable(address(0xCAFE));

    bytes32 constant IMAGE_ID = bytes32(uint256(0x1234));
    bytes32 constant LEZ_ZONE = bytes32(uint256(0x200e));
    bytes32 constant LEZ_PROGRAM = bytes32(uint256(0xb41d6e));
    bytes32 constant LEZ_INSTANCE = bytes32(uint256(0x1257a7ce));
    uint64 constant ANCHOR_DELAY = 1 hours;

    function setUp() public {
        verifier = new RiscZeroMockVerifier(bytes4(0));
        anchor = new StubLezAnchor(admin, ANCHOR_DELAY);
        vault = new Vault(verifier, anchor, IMAGE_ID, LEZ_ZONE, LEZ_PROGRAM, admin, 1 days);
        usdc = new MockToken("USDC", 6);
        usdt = new MockUsdt();
        feeToken = new MockFeeToken();

        vm.startPrank(admin);
        vault.bindLezInstance(LEZ_INSTANCE);
        vault.registerToken(vault.NATIVE(), _denoms(0.01 ether, 0.1 ether), 10 ether, 10 ether);
        vault.registerToken(address(usdc), _denoms(100e6, 1000e6), 1_000_000e6, 1_000_000e6);
        vault.registerToken(address(usdt), _denoms(100e6, 1000e6), 1_000_000e6, 1_000_000e6);
        vault.registerToken(address(feeToken), _denoms(100e18), 1_000_000e18, 1_000_000e18);
        vm.stopPrank();

        vm.deal(user, 100 ether);
        usdc.mint(user, 10_000e6);
        usdt.mint(user, 10_000e6);
        feeToken.mint(user, 10_000e18);
        vm.startPrank(user);
        usdc.approve(address(vault), type(uint256).max);
        usdt.approve(address(vault), type(uint256).max);
        feeToken.approve(address(vault), type(uint256).max);
        vm.stopPrank();
    }

    // ---------------------------------------------------------------- helpers

    function _denoms(uint256 a) internal pure returns (uint256[] memory d) {
        d = new uint256[](1);
        d[0] = a;
    }

    function _denoms(uint256 a, uint256 b) internal pure returns (uint256[] memory d) {
        d = new uint256[](2);
        d[0] = a;
        d[1] = b;
    }

    /// Reference root: full binary tree of depth 32, leaves padded with zero, computed layer by layer.
    function _referenceRoot(bytes32[] memory leaves) internal pure returns (bytes32) {
        bytes32[] memory layer = leaves;
        bytes32 zero = bytes32(0);
        for (uint256 h = 0; h < 32; h++) {
            uint256 n = (layer.length + 1) / 2;
            if (n == 0) n = 1;
            bytes32[] memory next = new bytes32[](n);
            for (uint256 i = 0; i < n; i++) {
                bytes32 l = 2 * i < layer.length ? layer[2 * i] : zero;
                bytes32 r = 2 * i + 1 < layer.length ? layer[2 * i + 1] : zero;
                next[i] = sha256(abi.encodePacked(l, r));
            }
            layer = next;
            zero = sha256(abi.encodePacked(zero, zero));
        }
        return layer[0];
    }

    function _journal(address token, uint256 denom, bytes32 nullifier, uint256 fee, bytes32 root)
        internal
        view
        returns (Vault.BurnJournal memory j)
    {
        j = Vault.BurnJournal({
            version: 1,
            chainId: block.chainid,
            vault: address(vault),
            lezProgram: LEZ_PROGRAM,
            lezInstance: LEZ_INSTANCE,
            accumulatorRoot: root,
            nullifier: nullifier,
            recipient: recipient,
            token: token,
            denom: denom,
            fee: fee
        });
    }

    function _seal(bytes memory journal) internal view returns (bytes memory) {
        return verifier.mockProve(IMAGE_ID, sha256(journal)).seal;
    }

    function _postRoot(bytes32 root) internal {
        vm.prank(admin);
        anchor.post(LEZ_INSTANCE, root);
        vm.warp(block.timestamp + ANCHOR_DELAY);
    }

    function _fundVaultEth() internal {
        vm.prank(user);
        vault.deposit{value: 0.1 ether}(address(0), 1, keccak256("c0"));
    }

    // ---------------------------------------------------------------- deposit

    function test_zeroHashesAreTheSha256Chain() public view {
        bytes32 z = bytes32(0);
        for (uint256 h = 0; h < 32; h++) {
            assertEq(vault.zeroHash(h), z);
            z = sha256(abi.encodePacked(z, z));
        }
    }

    function test_emptyRootMatchesReference() public view {
        assertEq(vault.depositRoot(), _referenceRoot(new bytes32[](0)));
    }

    function test_depositEthAppendsLeafAndRoot() public {
        bytes32 c = keccak256("commitment-1");
        bytes32 leaf = vault.leafHash(address(0), 0.01 ether, c);
        bytes32[] memory leaves = new bytes32[](1);
        leaves[0] = leaf;
        bytes32 expectedRoot = _referenceRoot(leaves);

        vm.expectEmit(true, false, false, true, address(vault));
        emit Vault.Deposit(address(0), 0.01 ether, c, 0, expectedRoot);
        vm.prank(user);
        vault.deposit{value: 0.01 ether}(address(0), 0, c);

        assertEq(vault.depositRoot(), expectedRoot);
        assertEq(vault.depositCount(), 1);
        assertEq(address(vault).balance, 0.01 ether);
    }

    function test_rootAtFixedSlot() public {
        vm.prank(user);
        vault.deposit{value: 0.01 ether}(address(0), 0, keccak256("x"));
        assertEq(vm.load(address(vault), vault.DEPOSIT_ROOT_SLOT()), vault.depositRoot());
        assertEq(vault.DEPOSIT_ROOT_SLOT(), bytes32(uint256(keccak256("beacon-veil.vault.depositRoot")) - 1));
    }

    function test_manyDepositsMatchReference() public {
        bytes32[] memory leaves = new bytes32[](9);
        for (uint256 i = 0; i < 9; i++) {
            bytes32 c = keccak256(abi.encode(i));
            if (i % 3 == 0) {
                vm.prank(user);
                vault.deposit{value: 0.1 ether}(address(0), 1, c);
                leaves[i] = vault.leafHash(address(0), 0.1 ether, c);
            } else if (i % 3 == 1) {
                vm.prank(user);
                vault.deposit(address(usdc), 0, c);
                leaves[i] = vault.leafHash(address(usdc), 100e6, c);
            } else {
                vm.prank(user);
                vault.deposit(address(usdt), 1, c);
                leaves[i] = vault.leafHash(address(usdt), 1000e6, c);
            }
            bytes32[] memory prefix = new bytes32[](i + 1);
            for (uint256 k = 0; k <= i; k++) {
                prefix[k] = leaves[k];
            }
            assertEq(vault.depositRoot(), _referenceRoot(prefix));
        }
    }

    function test_depositErc20() public {
        vm.prank(user);
        vault.deposit(address(usdc), 1, keccak256("c"));
        assertEq(usdc.balanceOf(address(vault)), 1000e6);
        assertEq(usdc.balanceOf(user), 9000e6);
    }

    /// F17: transferFrom returns nothing.
    function test_depositUsdtStyleToken() public {
        vm.prank(user);
        vault.deposit(address(usdt), 0, keccak256("c"));
        assertEq(usdt.balanceOf(address(vault)), 100e6);
    }

    function test_rejectsFeeOnTransferToken() public {
        vm.prank(user);
        vm.expectRevert(Vault.FeeOnTransferNotSupported.selector);
        vault.deposit(address(feeToken), 0, keccak256("c"));
    }

    function test_rejectsWrongEthValue() public {
        vm.prank(user);
        vm.expectRevert(Vault.WrongValue.selector);
        vault.deposit{value: 0.02 ether}(address(0), 0, keccak256("c"));
    }

    function test_rejectsEthWithErc20() public {
        vm.prank(user);
        vm.expectRevert(Vault.WrongValue.selector);
        vault.deposit{value: 1}(address(usdc), 0, keccak256("c"));
    }

    function test_rejectsUnregisteredToken() public {
        vm.prank(user);
        vm.expectRevert(Vault.UnsupportedToken.selector);
        vault.deposit(address(0xdead), 0, keccak256("c"));
    }

    function test_rejectsBadDenomIndex() public {
        vm.prank(user);
        vm.expectRevert(Vault.InvalidDenomination.selector);
        vault.deposit{value: 0.01 ether}(address(0), 2, keccak256("c"));
    }

    function test_plainEthTransferRefused() public {
        vm.prank(user);
        (bool ok,) = address(vault).call{value: 1 ether}("");
        assertFalse(ok);
    }

    function test_depositCapPerWindow() public {
        vm.prank(admin);
        vault.registerToken(address(0), _denoms(0.01 ether, 0.1 ether), 0.1 ether, 10 ether);
        vm.startPrank(user);
        vault.deposit{value: 0.1 ether}(address(0), 1, keccak256("a"));
        vm.expectRevert(Vault.CapExceeded.selector);
        vault.deposit{value: 0.01 ether}(address(0), 0, keccak256("b"));
        vm.warp(block.timestamp + 1 days);
        vault.deposit{value: 0.01 ether}(address(0), 0, keccak256("b"));
        vm.stopPrank();
    }

    function test_freezeDeposits() public {
        vm.prank(admin);
        vault.setTokenFreeze(address(usdc), true, false);
        vm.prank(user);
        vm.expectRevert(Vault.DepositsFrozen.selector);
        vault.deposit(address(usdc), 0, keccak256("c"));
        // other tokens unaffected
        vm.prank(user);
        vault.deposit{value: 0.01 ether}(address(0), 0, keccak256("c"));

        vm.prank(admin);
        vault.setGlobalFreeze(true, false);
        vm.prank(user);
        vm.expectRevert(Vault.DepositsFrozen.selector);
        vault.deposit{value: 0.01 ether}(address(0), 0, keccak256("d"));
    }

    function test_deregisterStopsDepositsButNotReleases() public {
        _fundVaultEth();
        vm.prank(admin);
        vault.deregisterToken(address(0));
        vm.prank(user);
        vm.expectRevert(Vault.UnsupportedToken.selector);
        vault.deposit{value: 0.01 ether}(address(0), 0, keccak256("c"));

        bytes32 root = keccak256("acc");
        _postRoot(root);
        bytes memory journal = abi.encode(_journal(address(0), 0.1 ether, keccak256("n"), 0, root));
        vault.release(_seal(journal), journal);
        assertEq(recipient.balance, 0.1 ether);
    }

    function test_onlyAdminRegisters() public {
        bytes32 role = vault.DEFAULT_ADMIN_ROLE();
        vm.prank(user);
        vm.expectRevert(abi.encodeWithSelector(IAccessControl.AccessControlUnauthorizedAccount.selector, user, role));
        vault.registerToken(address(0x1), _denoms(1), 1, 1);
    }

    function test_onlyFreezerFreezes() public {
        bytes32 role = vault.FREEZER_ROLE();
        vm.prank(user);
        vm.expectRevert(abi.encodeWithSelector(IAccessControl.AccessControlUnauthorizedAccount.selector, user, role));
        vault.setGlobalFreeze(true, true);
    }

    function test_instanceBindsOnce() public {
        vm.prank(admin);
        vm.expectRevert(Vault.InstanceAlreadyBound.selector);
        vault.bindLezInstance(bytes32(uint256(1)));
    }

    // ---------------------------------------------------------------- release

    function test_releaseEthPaysRecipientAndRelayer() public {
        _fundVaultEth();
        bytes32 root = keccak256("acc");
        _postRoot(root);
        bytes memory journal = abi.encode(_journal(address(0), 0.1 ether, keccak256("n1"), 0.001 ether, root));
        assertEq(journal.length, vault.JOURNAL_LENGTH());
        bytes memory seal = _seal(journal);
        vm.prank(relayer);
        vault.release(seal, journal);
        assertEq(recipient.balance, 0.099 ether);
        assertEq(relayer.balance, 0.001 ether);
        assertTrue(vault.burnNullifierSpent(keccak256("n1")));
    }

    function test_releaseUsdtStyleToken() public {
        vm.prank(user);
        vault.deposit(address(usdt), 1, keccak256("c"));
        bytes32 root = keccak256("acc");
        _postRoot(root);
        bytes memory journal = abi.encode(_journal(address(usdt), 1000e6, keccak256("n"), 1e6, root));
        bytes memory seal = _seal(journal);
        vm.prank(relayer);
        vault.release(seal, journal);
        assertEq(usdt.balanceOf(recipient), 999e6);
        assertEq(usdt.balanceOf(relayer), 1e6);
        assertEq(usdt.balanceOf(address(vault)), 0);
    }

    function test_releaseReplayRefused() public {
        _fundVaultEth();
        _fundVaultEth();
        bytes32 root = keccak256("acc");
        _postRoot(root);
        bytes memory journal = abi.encode(_journal(address(0), 0.1 ether, keccak256("n"), 0, root));
        bytes memory seal = _seal(journal);
        vault.release(seal, journal);
        vm.expectRevert(Vault.AlreadyReleased.selector);
        vault.release(seal, journal);
    }

    function test_releaseRefusesUnpostedOrEarlyRoot() public {
        _fundVaultEth();
        bytes32 root = keccak256("acc");
        bytes memory journal = abi.encode(_journal(address(0), 0.1 ether, keccak256("n"), 0, root));
        bytes memory seal = _seal(journal);
        vm.expectRevert(Vault.RootNotFinal.selector);
        vault.release(seal, journal);
        vm.prank(admin);
        anchor.post(LEZ_INSTANCE, root);
        vm.expectRevert(Vault.RootNotFinal.selector);
        vault.release(seal, journal);
        vm.warp(block.timestamp + ANCHOR_DELAY);
        vault.release(seal, journal);
    }

    function test_releaseRefusesTamperedJournal() public {
        _fundVaultEth();
        bytes32 root = keccak256("acc");
        _postRoot(root);
        Vault.BurnJournal memory j = _journal(address(0), 0.1 ether, keccak256("n"), 0, root);
        bytes memory seal = _seal(abi.encode(j));
        j.recipient = address(0xBAD);
        bytes memory tampered = abi.encode(j);
        vm.expectRevert(VerificationFailed.selector);
        vault.release(seal, tampered);
    }

    function test_releaseRefusesWrongImage() public {
        _fundVaultEth();
        bytes32 root = keccak256("acc");
        _postRoot(root);
        bytes memory journal = abi.encode(_journal(address(0), 0.1 ether, keccak256("n"), 0, root));
        bytes memory seal = verifier.mockProve(bytes32(uint256(0x999)), sha256(journal)).seal;
        vm.expectRevert(VerificationFailed.selector);
        vault.release(seal, journal);
    }

    function test_releaseRefusesWrongPairing() public {
        _fundVaultEth();
        bytes32 root = keccak256("acc");
        _postRoot(root);
        Vault.BurnJournal memory base = _journal(address(0), 0.1 ether, keccak256("n"), 0, root);

        Vault.BurnJournal memory j = base;
        j.chainId = 1;
        _expectPairing(j);
        j = _journal(address(0), 0.1 ether, keccak256("n"), 0, root);
        j.vault = address(0x1);
        _expectPairing(j);
        j = _journal(address(0), 0.1 ether, keccak256("n"), 0, root);
        j.lezProgram = bytes32(uint256(7));
        _expectPairing(j);
        j = _journal(address(0), 0.1 ether, keccak256("n"), 0, root);
        j.lezInstance = bytes32(uint256(7));
        _expectPairing(j);
    }

    function _expectPairing(Vault.BurnJournal memory j) internal {
        bytes memory journal = abi.encode(j);
        bytes memory seal = _seal(journal);
        vm.expectRevert(Vault.WrongPairing.selector);
        vault.release(seal, journal);
    }

    function test_releaseRefusesBadLengthOrVersion() public {
        bytes memory journal = abi.encode(_journal(address(0), 0.1 ether, keccak256("n"), 0, bytes32(0)));
        bytes memory short = new bytes(journal.length - 1);
        vm.expectRevert(Vault.BadJournal.selector);
        vault.release(hex"00000000", short);
        Vault.BurnJournal memory j = _journal(address(0), 0.1 ether, keccak256("n"), 0, bytes32(0));
        j.version = 2;
        bytes memory v2 = abi.encode(j);
        bytes memory seal = _seal(v2);
        vm.expectRevert(Vault.BadJournal.selector);
        vault.release(seal, v2);
    }

    function test_releaseRefusesUnknownDenomOrFee() public {
        _fundVaultEth();
        bytes32 root = keccak256("acc");
        _postRoot(root);
        bytes memory journal = abi.encode(_journal(address(0), 0.05 ether, keccak256("n"), 0, root));
        bytes memory seal = _seal(journal);
        vm.expectRevert(Vault.InvalidDenomination.selector);
        vault.release(seal, journal);
        journal = abi.encode(_journal(address(0), 0.1 ether, keccak256("n"), 0.1 ether, root));
        seal = _seal(journal);
        vm.expectRevert(Vault.InvalidDenomination.selector);
        vault.release(seal, journal);
    }

    function test_releaseFrozen() public {
        _fundVaultEth();
        bytes32 root = keccak256("acc");
        _postRoot(root);
        bytes memory journal = abi.encode(_journal(address(0), 0.1 ether, keccak256("n"), 0, root));
        bytes memory seal = _seal(journal);
        vm.prank(admin);
        vault.setGlobalFreeze(false, true);
        vm.expectRevert(Vault.ReleasesFrozen.selector);
        vault.release(seal, journal);
        vm.prank(admin);
        vault.setGlobalFreeze(false, false);
        vm.prank(admin);
        vault.setTokenFreeze(address(0), false, true);
        vm.expectRevert(Vault.ReleasesFrozen.selector);
        vault.release(seal, journal);
    }

    function test_releaseCap() public {
        _fundVaultEth();
        _fundVaultEth();
        vm.prank(admin);
        vault.registerToken(address(0), _denoms(0.01 ether, 0.1 ether), 10 ether, 0.1 ether);
        bytes32 root = keccak256("acc");
        _postRoot(root);
        bytes memory j1 = abi.encode(_journal(address(0), 0.1 ether, keccak256("n1"), 0, root));
        bytes memory j2 = abi.encode(_journal(address(0), 0.1 ether, keccak256("n2"), 0, root));
        vault.release(_seal(j1), j1);
        bytes memory s2 = _seal(j2);
        vm.expectRevert(Vault.CapExceeded.selector);
        vault.release(s2, j2);
        vm.warp(block.timestamp + 1 days);
        vault.release(s2, j2);
    }

    /// R2: a failed release keeps the entitlement (nullifier unspent).
    function test_failedReleaseKeepsEntitlement() public {
        _fundVaultEth();
        bytes32 root = keccak256("acc");
        _postRoot(root);
        RejectEth bad = new RejectEth();
        Vault.BurnJournal memory j = _journal(address(0), 0.1 ether, keccak256("n"), 0, root);
        j.recipient = address(bad);
        bytes memory journal = abi.encode(j);
        bytes memory seal = _seal(journal);
        vm.expectRevert(Vault.EthTransferFailed.selector);
        vault.release(seal, journal);
        assertFalse(vault.burnNullifierSpent(keccak256("n")));
    }

    function test_releaseNeedsBoundInstance() public {
        Vault fresh = new Vault(verifier, anchor, IMAGE_ID, LEZ_ZONE, LEZ_PROGRAM, admin, 1 days);
        bytes memory journal = abi.encode(_journal(address(0), 0.1 ether, keccak256("n"), 0, bytes32(0)));
        vm.expectRevert(Vault.InstanceNotBound.selector);
        fresh.release(hex"00000000", journal);
    }

    function test_stubAnchorOnlyOwnerPosts() public {
        vm.prank(user);
        vm.expectRevert();
        anchor.post(LEZ_INSTANCE, keccak256("r"));
    }
}
