// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {Test} from "forge-std/Test.sol";
import {ECDSA} from "@openzeppelin/contracts/utils/cryptography/ECDSA.sol";
import {CommitteeLezAnchor} from "../src/CommitteeLezAnchor.sol";

/// `CommitteeLezAnchor`: a fixed set of 4 test keys, threshold 3 (LEZ's own rule for 4 keys).
contract CommitteeLezAnchorTest is Test {
    CommitteeLezAnchor anchor;

    uint256[4] pks; // sorted by address
    address[4] addrs;
    uint256 outsiderPk = 0x0751DE;

    bytes32 constant ZONE = keccak256("lez-rc3-standalone");
    uint64 constant DELAY = 600;
    uint64 constant TIP_ID = 6593;
    bytes32 constant TIP_HASH = keccak256("tip");
    bytes32 constant INSTANCE = keccak256("instance");
    bytes32 constant ROOT = keccak256("root");
    uint256 constant SECP256K1_N = 0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEBAAEDCE6AF48A03BBFD25E8CD0364141;

    function setUp() public {
        uint256[4] memory raw = [uint256(0xA11), 0xB22, 0xC33, 0xD44];
        // Sort the keys by their address (insertion sort over 4).
        for (uint256 i = 0; i < 4; i++) {
            pks[i] = raw[i];
            addrs[i] = vm.addr(raw[i]);
        }
        for (uint256 i = 1; i < 4; i++) {
            for (uint256 j = i; j > 0 && addrs[j] < addrs[j - 1]; j--) {
                (addrs[j], addrs[j - 1]) = (addrs[j - 1], addrs[j]);
                (pks[j], pks[j - 1]) = (pks[j - 1], pks[j]);
            }
        }
        anchor = _deploy(3);
    }

    // ---------------------------------------------------------------- helpers

    function _set() internal view returns (address[] memory s) {
        s = new address[](4);
        for (uint256 i = 0; i < 4; i++) {
            s[i] = addrs[i];
        }
    }

    function _deploy(uint8 t) internal returns (CommitteeLezAnchor) {
        return new CommitteeLezAnchor(_set(), t, DELAY, ZONE);
    }

    function _sign(uint256 pk, bytes32 digest) internal pure returns (bytes memory) {
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(pk, digest);
        return abi.encodePacked(r, s, v);
    }

    /// Signatures of the members at `idx` (ascending indices give ascending signers).
    function _sigs(bytes32 digest, uint256[] memory idx) internal view returns (bytes[] memory sigs) {
        sigs = new bytes[](idx.length);
        for (uint256 i = 0; i < idx.length; i++) {
            sigs[i] = _sign(pks[idx[i]], digest);
        }
    }

    function _idx(uint256 a, uint256 b, uint256 c) internal pure returns (uint256[] memory r) {
        r = new uint256[](3);
        (r[0], r[1], r[2]) = (a, b, c);
    }

    function _idx(uint256 a, uint256 b) internal pure returns (uint256[] memory r) {
        r = new uint256[](2);
        (r[0], r[1]) = (a, b);
    }

    function _tipSigs(uint256[] memory idx) internal view returns (bytes[] memory) {
        return _sigs(anchor.tipDigest(TIP_ID, TIP_HASH), idx);
    }

    // ---------------------------------------------------------------- tips

    function test_threeOfFourAttestsAndIsFinalAfterDelay() public {
        vm.expectEmit(address(anchor));
        emit CommitteeLezAnchor.TipAttested(TIP_ID, TIP_HASH, uint64(vm.getBlockTimestamp()) + DELAY, 3);
        anchor.attestTip(TIP_ID, TIP_HASH, _tipSigs(_idx(0, 1, 3)));
        assertFalse(anchor.isFinalTip(TIP_ID, TIP_HASH));
        vm.warp(vm.getBlockTimestamp() + DELAY - 1);
        assertFalse(anchor.isFinalTip(TIP_ID, TIP_HASH));
        vm.warp(vm.getBlockTimestamp() + 1);
        assertTrue(anchor.isFinalTip(TIP_ID, TIP_HASH));
        assertFalse(anchor.isFinalTip(TIP_ID, keccak256("other")));
        assertFalse(anchor.isFinalTip(TIP_ID + 1, TIP_HASH));
    }

    function test_allFourAttest() public {
        uint256[] memory all = new uint256[](4);
        (all[0], all[1], all[2], all[3]) = (0, 1, 2, 3);
        anchor.attestTip(TIP_ID, TIP_HASH, _tipSigs(all));
        vm.warp(vm.getBlockTimestamp() + DELAY);
        assertTrue(anchor.isFinalTip(TIP_ID, TIP_HASH));
    }

    function test_anyoneMaySubmit() public {
        bytes[] memory sigs = _tipSigs(_idx(1, 2, 3));
        vm.prank(address(0xBEEF));
        anchor.attestTip(TIP_ID, TIP_HASH, sigs);
        assertGt(anchor.tipAttestedAt(TIP_ID, TIP_HASH), 0);
    }

    function test_refusesTooFewSignatures() public {
        bytes[] memory sigs = _tipSigs(_idx(0, 1));
        vm.expectRevert(CommitteeLezAnchor.TooFewSignatures.selector);
        anchor.attestTip(TIP_ID, TIP_HASH, sigs);
    }

    function test_refusesOutsideSigner() public {
        bytes32 d = anchor.tipDigest(TIP_ID, TIP_HASH);
        bytes[] memory sigs = _sigs(d, _idx(0, 1, 2));
        sigs[2] = _sign(outsiderPk, d);
        vm.expectRevert(abi.encodeWithSelector(CommitteeLezAnchor.NotASigner.selector, vm.addr(outsiderPk)));
        anchor.attestTip(TIP_ID, TIP_HASH, sigs);
    }

    function test_refusesSameSignerTwice() public {
        bytes32 d = anchor.tipDigest(TIP_ID, TIP_HASH);
        bytes[] memory sigs = _sigs(d, _idx(0, 1, 1));
        vm.expectRevert(CommitteeLezAnchor.SignersNotAscending.selector);
        anchor.attestTip(TIP_ID, TIP_HASH, sigs);
        // Four signatures where one member appears twice: still refused, even with the threshold met by the others.
        uint256[] memory four = new uint256[](4);
        (four[0], four[1], four[2], four[3]) = (0, 1, 2, 2);
        sigs = _sigs(d, four);
        vm.expectRevert(CommitteeLezAnchor.SignersNotAscending.selector);
        anchor.attestTip(TIP_ID, TIP_HASH, sigs);
    }

    function test_refusesUnsortedSigners() public {
        bytes[] memory sigs = _tipSigs(_idx(2, 0, 1));
        vm.expectRevert(CommitteeLezAnchor.SignersNotAscending.selector);
        anchor.attestTip(TIP_ID, TIP_HASH, sigs);
    }

    function test_refusesSignaturesForAnotherTip() public {
        bytes[] memory sigs = _tipSigs(_idx(0, 1, 2));
        // Same signatures under another hash or another block id recover to other addresses.
        vm.expectRevert();
        anchor.attestTip(TIP_ID, keccak256("other"), sigs);
        vm.expectRevert();
        anchor.attestTip(TIP_ID - 1, TIP_HASH, sigs);
        assertEq(anchor.tipAttestedAt(TIP_ID, keccak256("other")), 0);
        assertEq(anchor.tipAttestedAt(TIP_ID - 1, TIP_HASH), 0);
    }

    function _expectNotASigner(CommitteeLezAnchor a, bytes32 digestSigned, bytes[] memory sigs) internal {
        // The first signature recovers, under the anchor's own digest, to an address outside the set.
        address r = ECDSA.recover(a.tipDigest(TIP_ID, TIP_HASH), sigs[0]);
        assertFalse(a.isSigner(r));
        assertTrue(digestSigned != a.tipDigest(TIP_ID, TIP_HASH));
        vm.expectRevert(abi.encodeWithSelector(CommitteeLezAnchor.NotASigner.selector, r));
        a.attestTip(TIP_ID, TIP_HASH, sigs);
    }

    function test_refusesSignaturesForAnotherAnchor() public {
        CommitteeLezAnchor other = _deploy(3); // same keys, another address
        bytes32 d = other.tipDigest(TIP_ID, TIP_HASH);
        bytes[] memory sigs = _sigs(d, _idx(0, 1, 2));
        other.attestTip(TIP_ID, TIP_HASH, sigs);
        _expectNotASigner(anchor, d, sigs);
    }

    function test_refusesSignaturesForAnotherChain() public {
        uint256 home = block.chainid;
        vm.chainId(1);
        bytes32 d = anchor.tipDigest(TIP_ID, TIP_HASH);
        bytes[] memory sigs = _sigs(d, _idx(0, 1, 2));
        vm.chainId(home);
        _expectNotASigner(anchor, d, sigs);
    }

    function test_refusesTipSignaturesAsRoot() public {
        // Tip(zone, id, hash) signatures, with the same words submitted as Root(zone, instance, root).
        bytes32 asInstance = bytes32(uint256(TIP_ID));
        bytes[] memory sigs = _tipSigs(_idx(0, 1, 2));
        vm.expectRevert();
        anchor.attestRoot(asInstance, TIP_HASH, sigs);
        assertEq(anchor.rootAttestedAt(asInstance, TIP_HASH), 0);
    }

    function test_refusesRootSignaturesAsTip() public {
        bytes32 asInstance = bytes32(uint256(TIP_ID));
        bytes[] memory sigs = _sigs(anchor.rootDigest(asInstance, TIP_HASH), _idx(0, 1, 2));
        anchor.attestRoot(asInstance, TIP_HASH, sigs); // valid as a root
        vm.expectRevert();
        anchor.attestTip(TIP_ID, TIP_HASH, sigs);
        assertEq(anchor.tipAttestedAt(TIP_ID, TIP_HASH), 0);
    }

    function test_refusesRepeatAttestation() public {
        bytes[] memory sigs = _tipSigs(_idx(0, 1, 2));
        anchor.attestTip(TIP_ID, TIP_HASH, sigs);
        uint64 t = anchor.tipAttestedAt(TIP_ID, TIP_HASH);
        vm.warp(vm.getBlockTimestamp() + 100);
        sigs = _tipSigs(_idx(1, 2, 3));
        vm.expectRevert(CommitteeLezAnchor.AlreadyAttested.selector);
        anchor.attestTip(TIP_ID, TIP_HASH, sigs);
        assertEq(anchor.tipAttestedAt(TIP_ID, TIP_HASH), t); // the delay cannot be restarted
    }

    function test_refusesHighSSignature() public {
        bytes32 d = anchor.tipDigest(TIP_ID, TIP_HASH);
        bytes[] memory sigs = _sigs(d, _idx(0, 1, 2));
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(pks[0], d);
        // The malleable twin (r, n - s, v ^ 1) recovers to the same key; ECDSA.recover refuses it.
        bytes32 highS = bytes32(SECP256K1_N - uint256(s));
        sigs[0] = abi.encodePacked(r, highS, v == 27 ? uint8(28) : uint8(27));
        vm.expectRevert(abi.encodeWithSelector(ECDSA.ECDSAInvalidSignatureS.selector, highS));
        anchor.attestTip(TIP_ID, TIP_HASH, sigs);
    }

    function test_refusesMalformedSignature() public {
        bytes[] memory sigs = _tipSigs(_idx(0, 1, 2));
        sigs[1] = bytes.concat(sigs[1], bytes1(0)); // 66 bytes
        vm.expectRevert(abi.encodeWithSelector(ECDSA.ECDSAInvalidSignatureLength.selector, uint256(66)));
        anchor.attestTip(TIP_ID, TIP_HASH, sigs);
        sigs = _tipSigs(_idx(0, 1, 2));
        sigs[1] = new bytes(65); // r = s = v = 0
        vm.expectRevert(ECDSA.ECDSAInvalidSignature.selector);
        anchor.attestTip(TIP_ID, TIP_HASH, sigs);
    }

    function test_conflictingTipVoidsTheHeight() public {
        anchor.attestTip(TIP_ID, TIP_HASH, _tipSigs(_idx(0, 1, 2)));
        bytes32 fork = keccak256("fork");
        bytes[] memory sigs = _sigs(anchor.tipDigest(TIP_ID, fork), _idx(1, 2, 3));
        vm.expectEmit(address(anchor));
        emit CommitteeLezAnchor.TipConflict(TIP_ID, TIP_HASH, fork);
        anchor.attestTip(TIP_ID, fork, sigs);
        assertTrue(anchor.tipVoid(TIP_ID));
        vm.warp(vm.getBlockTimestamp() + DELAY);
        assertFalse(anchor.isFinalTip(TIP_ID, TIP_HASH));
        assertFalse(anchor.isFinalTip(TIP_ID, fork));
        // Other heights are not affected.
        anchor.attestTip(TIP_ID + 1, TIP_HASH, _sigs(anchor.tipDigest(TIP_ID + 1, TIP_HASH), _idx(0, 1, 2)));
        vm.warp(vm.getBlockTimestamp() + DELAY);
        assertTrue(anchor.isFinalTip(TIP_ID + 1, TIP_HASH));
    }

    // ---------------------------------------------------------------- the set

    function test_constructorRefusesBadSets() public {
        address[] memory s = _set();
        address[] memory two = new address[](2);
        (two[0], two[1]) = (s[0], s[1]);
        vm.expectRevert(CommitteeLezAnchor.BadSignerSet.selector);
        new CommitteeLezAnchor(two, 1, DELAY, ZONE);

        address[] memory many = new address[](17);
        for (uint256 i = 0; i < 17; i++) {
            many[i] = address(uint160(i + 1));
        }
        vm.expectRevert(CommitteeLezAnchor.BadSignerSet.selector);
        new CommitteeLezAnchor(many, 11, DELAY, ZONE);

        address[] memory zero = _set();
        zero[0] = address(0);
        vm.expectRevert(CommitteeLezAnchor.BadSignerSet.selector);
        new CommitteeLezAnchor(zero, 3, DELAY, ZONE);

        address[] memory unsorted = _set();
        (unsorted[0], unsorted[1]) = (unsorted[1], unsorted[0]);
        vm.expectRevert(CommitteeLezAnchor.BadSignerSet.selector);
        new CommitteeLezAnchor(unsorted, 3, DELAY, ZONE);

        address[] memory repeated = _set();
        repeated[2] = repeated[1];
        vm.expectRevert(CommitteeLezAnchor.BadSignerSet.selector);
        new CommitteeLezAnchor(repeated, 3, DELAY, ZONE);

        vm.expectRevert(CommitteeLezAnchor.BadSignerSet.selector);
        new CommitteeLezAnchor(s, 2, DELAY, ZONE); // below LEZ's rule (3 for 4 keys)
        vm.expectRevert(CommitteeLezAnchor.BadSignerSet.selector);
        new CommitteeLezAnchor(s, 5, DELAY, ZONE); // above n

        // The edges that pass: 4 of 4, and 16 keys at LEZ's threshold.
        assertEq(new CommitteeLezAnchor(s, 4, DELAY, ZONE).threshold(), 4);
        address[] memory sixteen = new address[](16);
        for (uint256 i = 0; i < 16; i++) {
            sixteen[i] = address(uint160(i + 1));
        }
        assertEq(new CommitteeLezAnchor(sixteen, 11, DELAY, ZONE).signers().length, 16);
    }

    function test_setIsFixed() public view {
        address[] memory s = anchor.signers();
        assertEq(s.length, 4);
        for (uint256 i = 0; i < 4; i++) {
            assertEq(s[i], addrs[i]);
            assertTrue(anchor.isSigner(addrs[i]));
        }
        assertFalse(anchor.isSigner(vm.addr(outsiderPk)));
        assertEq(anchor.threshold(), 3);
        assertEq(anchor.delay(), DELAY);
        assertEq(anchor.zone(), ZONE);
    }

    /// LEZ v0.4.0-rc3, lez/sequencer/core/src/committee_discovery.rs, test
    /// `no_single_key_reconfigures_a_committee_of_three_or_more`.
    function test_lezThresholdMatchesLez() public view {
        uint256[9] memory want = [uint256(1), 1, 2, 3, 4, 4, 5, 6, 6];
        for (uint256 n = 1; n <= 9; n++) {
            assertEq(anchor.lezThreshold(n), want[n - 1], "n");
        }
        assertEq(anchor.lezThreshold(16), 11);
    }

    function testFuzz_fewerThanThresholdNeverAttests(uint64 blockId, bytes32 blockHash, uint8 mask) public {
        // Any subset of fewer than 3 members, in ascending order, is refused.
        uint256 m = mask & 0x0f;
        uint256 k;
        for (uint256 i = 0; i < 4; i++) {
            if (m & (1 << i) != 0) k++;
        }
        vm.assume(k < 3);
        bytes32 d = anchor.tipDigest(blockId, blockHash);
        bytes[] memory sigs = new bytes[](k);
        uint256 j;
        for (uint256 i = 0; i < 4; i++) {
            if (m & (1 << i) != 0) sigs[j++] = _sign(pks[i], d);
        }
        vm.expectRevert(CommitteeLezAnchor.TooFewSignatures.selector);
        anchor.attestTip(blockId, blockHash, sigs);
        assertFalse(anchor.isFinalTip(blockId, blockHash));
    }

    // ---------------------------------------------------------------- roots

    function test_rootThreeOfFourAttests() public {
        bytes[] memory sigs = _sigs(anchor.rootDigest(INSTANCE, ROOT), _idx(0, 2, 3));
        anchor.attestRoot(INSTANCE, ROOT, sigs);
        assertFalse(anchor.isFinal(INSTANCE, ROOT));
        vm.warp(vm.getBlockTimestamp() + DELAY);
        assertTrue(anchor.isFinal(INSTANCE, ROOT));
        assertFalse(anchor.isFinal(keccak256("other instance"), ROOT));
        vm.expectRevert(CommitteeLezAnchor.AlreadyAttested.selector);
        anchor.attestRoot(INSTANCE, ROOT, sigs);
    }

    function test_rootRefusesTooFew() public {
        bytes[] memory sigs = _sigs(anchor.rootDigest(INSTANCE, ROOT), _idx(0, 2));
        vm.expectRevert(CommitteeLezAnchor.TooFewSignatures.selector);
        anchor.attestRoot(INSTANCE, ROOT, sigs);
    }
}
