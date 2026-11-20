// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {Test} from "forge-std/Test.sol";
import {VerificationFailed} from "risc0/IRiscZeroVerifier.sol";
import {RiscZeroMockVerifier} from "risc0/test/RiscZeroMockVerifier.sol";
import {Ownable} from "@openzeppelin/contracts/access/Ownable.sol";
import {Vault} from "../src/Vault.sol";
import {VaultChain} from "../src/VaultChain.sol";
import {StubLezAnchor} from "../src/StubLezAnchor.sol";
import {StubLezTipAnchor} from "../src/StubLezTipAnchor.sol";
import {MockToken} from "./mocks/Tokens.sol";

/// The chain route of `VaultChain` with RISC Zero's mock verifier, and the journals guest `burn_chain` produced in the
/// executor on the synthetic fixture chain (evidence/bridge/chain-anchor/journal-synthetic-{private,public}.hex).
contract VaultChainTest is Test {
    VaultChain vault;
    StubLezAnchor rootAnchor;
    StubLezTipAnchor tipAnchor;
    RiscZeroMockVerifier verifier;
    MockToken usdc;

    address admin = address(0xA11CE);
    address user = address(0xB0B);
    address relayer = address(0x5E1A7);
    address payable recipient = payable(address(0xCAFE));

    bytes32 constant BURN_IMAGE_ID = bytes32(uint256(0x1234));
    bytes32 constant CHAIN_IMAGE_ID = bytes32(uint256(0xc4a1));
    bytes32 constant LEZ_ZONE = bytes32(uint256(0x200e));
    bytes32 constant LEZ_PROGRAM = bytes32(uint256(0xb41d6e));
    bytes32 constant LEZ_INSTANCE = bytes32(uint256(0x1257a7ce));
    uint64 constant PINNED_ID = 2585;
    bytes32 constant PINNED_HASH = keccak256("pinned");
    uint64 constant TIP_ID = 2591;
    bytes32 constant TIP_HASH = keccak256("tip");
    uint64 constant DELAY = 1 hours;
    uint256 constant DENOM = 0.01 ether;
    uint256 constant FEE = 0.001 ether;

    function setUp() public {
        verifier = new RiscZeroMockVerifier(bytes4(0));
        rootAnchor = new StubLezAnchor(admin, DELAY);
        tipAnchor = new StubLezTipAnchor(admin, DELAY);
        vault = _deploy(LEZ_PROGRAM, PINNED_ID, PINNED_HASH);
        usdc = new MockToken("USDC", 6);
        vm.startPrank(admin);
        vault.bindLezInstance(LEZ_INSTANCE);
        vault.registerToken(vault.NATIVE(), _denoms(DENOM), 10 ether, 0.02 ether);
        vault.registerToken(address(usdc), _denoms(100e6), 1_000_000e6, 1_000_000e6);
        vm.stopPrank();
        vm.deal(user, 1 ether);
        vm.startPrank(user);
        for (uint256 i = 0; i < 4; i++) {
            vault.deposit{value: DENOM}(vault.NATIVE(), 0, bytes32(i));
        }
        vm.stopPrank();
    }

    // ---------------------------------------------------------------- helpers

    function _deploy(bytes32 program, uint64 pinnedId, bytes32 pinnedHash) internal returns (VaultChain) {
        return new VaultChain(
            verifier,
            rootAnchor,
            BURN_IMAGE_ID,
            LEZ_ZONE,
            program,
            admin,
            1 days,
            VaultChain.ChainRoute(tipAnchor, CHAIN_IMAGE_ID, pinnedId, pinnedHash)
        );
    }

    function _denoms(uint256 a) internal pure returns (uint256[] memory d) {
        d = new uint256[](1);
        d[0] = a;
    }

    function _j(bytes32 nullifier) internal view returns (VaultChain.ChainJournal memory) {
        return VaultChain.ChainJournal({
            version: 2,
            chainId: block.chainid,
            vault: address(vault),
            lezProgram: LEZ_PROGRAM,
            lezInstance: LEZ_INSTANCE,
            pinnedId: PINNED_ID,
            pinnedHash: PINNED_HASH,
            tipId: TIP_ID,
            tipHash: TIP_HASH,
            burnBlockId: 2588,
            nullifier: nullifier,
            recipient: recipient,
            token: address(0),
            denom: DENOM,
            fee: FEE
        });
    }

    function _enc(VaultChain.ChainJournal memory j) internal pure returns (bytes memory) {
        return abi.encode(j);
    }

    function _seal(bytes memory journal) internal view returns (bytes memory) {
        return verifier.mockProve(CHAIN_IMAGE_ID, sha256(journal)).seal;
    }

    function _attestTip(uint64 id, bytes32 h) internal {
        vm.prank(admin);
        tipAnchor.attestTip(id, h);
        vm.warp(vm.getBlockTimestamp() + DELAY);
    }

    function _release(bytes memory journal) internal {
        bytes memory seal = _seal(journal); // before the prank: mockProve is an external call
        vm.prank(relayer);
        vault.releaseChain(seal, journal);
    }

    function _expectRevert(bytes memory journal, bytes4 err) internal {
        bytes memory seal = _seal(journal);
        vm.expectRevert(err);
        vm.prank(relayer);
        vault.releaseChain(seal, journal);
    }

    // ---------------------------------------------------------------- the route

    function test_chainReleasePaysOnce() public {
        _attestTip(TIP_ID, TIP_HASH);
        bytes memory journal = _enc(_j(keccak256("n1")));
        assertEq(journal.length, vault.CHAIN_JOURNAL_LENGTH());
        uint256 r0 = recipient.balance;
        _release(journal);
        assertEq(recipient.balance - r0, DENOM - FEE);
        assertEq(relayer.balance, FEE);
        assertTrue(vault.burnNullifierSpent(keccak256("n1")));
        _expectRevert(journal, Vault.AlreadyReleased.selector);
    }

    function test_chainReleaseRefusesVersionOneOrBadLength() public {
        _attestTip(TIP_ID, TIP_HASH);
        VaultChain.ChainJournal memory j = _j(keccak256("n"));
        j.version = 1;
        _expectRevert(_enc(j), Vault.BadJournal.selector);
        // A version-1 burn_proof journal (11 words) on the chain route.
        bytes memory v1 = abi.encode(
            Vault.BurnJournal(
                1,
                block.chainid,
                address(vault),
                LEZ_PROGRAM,
                LEZ_INSTANCE,
                bytes32(0),
                keccak256("n"),
                recipient,
                address(0),
                DENOM,
                FEE
            )
        );
        _expectRevert(v1, Vault.BadJournal.selector);
        _expectRevert(bytes.concat(_enc(_j(keccak256("n"))), bytes1(0)), Vault.BadJournal.selector);
    }

    function test_chainReleaseRefusesWrongPinnedBlock() public {
        _attestTip(TIP_ID, TIP_HASH);
        VaultChain.ChainJournal memory j = _j(keccak256("n"));
        j.pinnedHash = keccak256("other chain");
        _expectRevert(_enc(j), VaultChain.WrongPinnedBlock.selector);
        j = _j(keccak256("n"));
        j.pinnedId = PINNED_ID + 1;
        _expectRevert(_enc(j), VaultChain.WrongPinnedBlock.selector);
    }

    function test_chainReleaseRefusesUnattestedOrEarlyTip() public {
        bytes memory journal = _enc(_j(keccak256("n")));
        _expectRevert(journal, VaultChain.TipNotFinal.selector);
        vm.prank(admin);
        tipAnchor.attestTip(TIP_ID, TIP_HASH);
        _expectRevert(journal, VaultChain.TipNotFinal.selector); // inside the delay
        vm.warp(vm.getBlockTimestamp() + DELAY - 1);
        _expectRevert(journal, VaultChain.TipNotFinal.selector);
        vm.warp(vm.getBlockTimestamp() + 1);
        _release(journal);
        // Another hash for the same block id, or the attested hash under another id, is not final.
        VaultChain.ChainJournal memory j = _j(keccak256("n2"));
        j.tipHash = keccak256("fork");
        _expectRevert(_enc(j), VaultChain.TipNotFinal.selector);
        j = _j(keccak256("n2"));
        j.tipId = TIP_ID + 1;
        _expectRevert(_enc(j), VaultChain.TipNotFinal.selector);
        j.tipId = uint256(type(uint64).max) + 1;
        _expectRevert(_enc(j), VaultChain.TipNotFinal.selector);
    }

    function test_chainReleaseRefusesWrongPairing() public {
        _attestTip(TIP_ID, TIP_HASH);
        VaultChain.ChainJournal memory j = _j(keccak256("n"));
        j.lezInstance = bytes32(uint256(0xbad));
        _expectRevert(_enc(j), Vault.WrongPairing.selector);
        j = _j(keccak256("n"));
        j.lezProgram = bytes32(uint256(0xbad));
        _expectRevert(_enc(j), Vault.WrongPairing.selector);
        j = _j(keccak256("n"));
        j.chainId = 1;
        _expectRevert(_enc(j), Vault.WrongPairing.selector);
        j = _j(keccak256("n"));
        j.vault = address(0xdead);
        _expectRevert(_enc(j), Vault.WrongPairing.selector);
    }

    function test_chainReleaseRefusesWrongImageOrTamperedJournal() public {
        _attestTip(TIP_ID, TIP_HASH);
        bytes memory journal = _enc(_j(keccak256("n")));
        // A seal for the burn_proof image does not verify against the chain image.
        bytes memory sealOther = verifier.mockProve(BURN_IMAGE_ID, sha256(journal)).seal;
        vm.expectRevert(VerificationFailed.selector);
        vault.releaseChain(sealOther, journal);
        // A seal for one journal does not cover another.
        bytes memory seal = _seal(journal);
        VaultChain.ChainJournal memory j = _j(keccak256("n"));
        j.recipient = address(0xbeef);
        vm.expectRevert(VerificationFailed.selector);
        vault.releaseChain(seal, _enc(j));
    }

    function test_chainReleaseRefusesUnknownDenomFeeOrToken() public {
        _attestTip(TIP_ID, TIP_HASH);
        VaultChain.ChainJournal memory j = _j(keccak256("n"));
        j.denom = 0.02 ether;
        _expectRevert(_enc(j), Vault.InvalidDenomination.selector);
        j = _j(keccak256("n"));
        j.fee = DENOM;
        _expectRevert(_enc(j), Vault.InvalidDenomination.selector);
        j = _j(keccak256("n"));
        j.token = address(0x7070);
        _expectRevert(_enc(j), Vault.UnsupportedToken.selector);
    }

    function test_nullifierIsSharedWithTheRootRoute() public {
        _attestTip(TIP_ID, TIP_HASH);
        // Root route first (burn_proof journal, owner-posted root), then the chain route with the same nullifier.
        bytes32 n = keccak256("shared");
        bytes32 root = keccak256("root");
        vm.prank(admin);
        rootAnchor.post(LEZ_INSTANCE, root);
        vm.warp(vm.getBlockTimestamp() + DELAY);
        bytes memory v1 = abi.encode(
            Vault.BurnJournal(
                1, block.chainid, address(vault), LEZ_PROGRAM, LEZ_INSTANCE, root, n, recipient, address(0), DENOM, FEE
            )
        );
        bytes memory sealA = verifier.mockProve(BURN_IMAGE_ID, sha256(v1)).seal;
        vm.prank(relayer);
        vault.release(sealA, v1);
        _expectRevert(_enc(_j(n)), Vault.AlreadyReleased.selector);
        // Chain route first, then the root route.
        bytes32 m = keccak256("shared2");
        _release(_enc(_j(m)));
        bytes memory v1b = abi.encode(
            Vault.BurnJournal(
                1, block.chainid, address(vault), LEZ_PROGRAM, LEZ_INSTANCE, root, m, recipient, address(0), DENOM, FEE
            )
        );
        bytes memory sealB = verifier.mockProve(BURN_IMAGE_ID, sha256(v1b)).seal;
        vm.expectRevert(Vault.AlreadyReleased.selector);
        vault.release(sealB, v1b);
    }

    function test_chainRouteFreezeAndCaps() public {
        _attestTip(TIP_ID, TIP_HASH);
        vm.prank(admin);
        vault.setChainRouteFreeze(true);
        _expectRevert(_enc(_j(keccak256("a"))), Vault.RouteFrozen.selector);
        vm.prank(admin);
        vault.setChainRouteFreeze(false);
        vm.prank(admin);
        vault.setRouteFreeze(true, false); // the zero-knowledge freeze stops this route too
        _expectRevert(_enc(_j(keccak256("a"))), Vault.RouteFrozen.selector);
        vm.prank(admin);
        vault.setRouteFreeze(false, false);
        vm.prank(user);
        vm.expectRevert();
        vault.setChainRouteFreeze(true);
        // Release cap 0.02 ether per window: two pass, the third waits for the next window.
        _release(_enc(_j(keccak256("a"))));
        _release(_enc(_j(keccak256("b"))));
        _expectRevert(_enc(_j(keccak256("c"))), Vault.CapExceeded.selector);
        vm.warp(vm.getBlockTimestamp() + 1 days);
        _release(_enc(_j(keccak256("c"))));
        // Global and token release freezes apply.
        vm.prank(admin);
        vault.setGlobalFreeze(false, true);
        _expectRevert(_enc(_j(keccak256("d"))), Vault.ReleasesFrozen.selector);
    }

    function test_chainReleaseNeedsBoundInstance() public {
        VaultChain fresh = _deploy(LEZ_PROGRAM, PINNED_ID, PINNED_HASH);
        _attestTip(TIP_ID, TIP_HASH);
        VaultChain.ChainJournal memory j = _j(keccak256("n"));
        j.vault = address(fresh);
        bytes memory journal = _enc(j);
        bytes memory seal = _seal(journal);
        vm.expectRevert(Vault.InstanceNotBound.selector);
        fresh.releaseChain(seal, journal);
    }

    function test_stubTipAnchorOnlyOwnerAttestsOnce() public {
        vm.prank(user);
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, user));
        tipAnchor.attestTip(TIP_ID, TIP_HASH);
        vm.prank(admin);
        tipAnchor.attestTip(TIP_ID, TIP_HASH);
        vm.prank(admin);
        vm.expectRevert(StubLezTipAnchor.AlreadyAttested.selector);
        tipAnchor.attestTip(TIP_ID, TIP_HASH);
        assertFalse(tipAnchor.isFinalTip(TIP_ID, TIP_HASH));
        vm.warp(vm.getBlockTimestamp() + DELAY);
        assertTrue(tipAnchor.isFinalTip(TIP_ID, TIP_HASH));
    }

    // ---------------------------------------------------------------- journals from guest burn_chain (executor)

    /// evidence/bridge/chain-anchor/journal-synthetic-public.hex: the synthetic public burn of fixtures/lez-chain,
    /// 1e8 units of the seq-dev config's ERC-20 test token to 0x7777...7777, fee 0, vault 0x380b...ad27 on Sepolia.
    bytes constant RUST_JOURNAL_PUBLIC =
        hex"00000000000000000000000000000000000000000000000000000000000000020000000000000000000000000000000000000000000000000000000000aa36a7000000000000000000000000380be74d9f3dfebaafc5f911cd6a45bfbee3ad2761c82e1d2abcd2a735763cfc988164881a5a22ebc0022e6af275ae99347be64e9b7354d99185285a50471440e4f21809ffe9fa18bbc0a605007b7b891b3993490000000000000000000000000000000000000000000000000000000000000a193c5b777451146a86ee71dc0783169b18ea64243c40de1ca98a005332e8cf7e5a0000000000000000000000000000000000000000000000000000000000000a1f80424c6e52d67274bcd42df99ef506ed09ce2503fef9ef16c8fc3de1065a321f0000000000000000000000000000000000000000000000000000000000000a1ef2f67785eb9b6763128c0c6dceb029ee94ee69a51198a72a07f51d8fda745eeb0000000000000000000000007777777777777777777777777777777777777777000000000000000000000000001b4d55b24a5ebf4e4e0d9cbf964e6170169e2b0000000000000000000000000000000000000000000000000000000005f5e1000000000000000000000000000000000000000000000000000000000000000000";

    function test_guestJournalDecodesAndPays() public {
        bytes memory journal = RUST_JOURNAL_PUBLIC;
        VaultChain.ChainJournal memory j = abi.decode(journal, (VaultChain.ChainJournal));
        assertEq(j.version, 2);
        assertEq(j.pinnedId, 2585);
        assertEq(j.tipId, 2591);
        assertEq(j.burnBlockId, 2590);
        assertEq(j.recipient, address(0x7777777777777777777777777777777777777777));
        assertEq(j.denom, 100e6);

        // The vault at the journal's address on the journal's chain, pinned at the journal's block 2585.
        vm.chainId(j.chainId);
        deployCodeTo(
            "VaultChain.sol:VaultChain",
            abi.encode(
                verifier,
                rootAnchor,
                BURN_IMAGE_ID,
                LEZ_ZONE,
                j.lezProgram,
                admin,
                1 days,
                VaultChain.ChainRoute(tipAnchor, CHAIN_IMAGE_ID, uint64(j.pinnedId), j.pinnedHash)
            ),
            j.vault
        );
        VaultChain v = VaultChain(payable(j.vault));
        deployCodeTo("Tokens.sol:MockToken", abi.encode("TUSDT", uint8(6)), j.token);
        MockToken(j.token).mint(j.vault, 100e6);
        vm.startPrank(admin);
        v.bindLezInstance(j.lezInstance);
        v.registerToken(j.token, _denoms(100e6), 1_000_000e6, 1_000_000e6);
        tipAnchor.attestTip(uint64(j.tipId), j.tipHash);
        vm.stopPrank();
        vm.warp(vm.getBlockTimestamp() + DELAY);

        v.releaseChain(_seal(journal), journal);
        assertEq(MockToken(j.token).balanceOf(j.recipient), 100e6);
        assertTrue(v.burnNullifierSpent(j.nullifier));
    }
}
