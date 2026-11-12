// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {SafeERC20} from "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
import {ReentrancyGuard} from "@openzeppelin/contracts/utils/ReentrancyGuard.sol";
import {AccessControl} from "@openzeppelin/contracts/access/AccessControl.sol";
import {IRiscZeroVerifier} from "risc0/IRiscZeroVerifier.sol";
import {ILezAnchor} from "./ILezAnchor.sol";

/// @title Vault (RFP-022 spike, owner W5)
/// @notice Ethereum side of the privacy-preserving bridge (PLAN.md 2.4, docs/bridge/).
///
/// Inbound: `deposit(token, denomIndex, commitment)` escrows exactly one permitted denomination of native ETH or a
/// registered ERC-20 and appends `leaf = sha256("wb/leaf/v1" || token || denom || commitment)` to an incremental
/// SHA-256 Merkle tree of depth 32 (the algorithm of the Ethereum deposit contract, CC0). The current root sits in one
/// fixed storage slot, `DEPOSIT_ROOT_SLOT`, which the LEZ bridge proves through RFP-021 (a storage claim). The
/// commitment hides the LEZ destination (F2); nothing in the call, the event or the state names it.
///
/// Outbound: `release(seal, journal)` pays out against a RISC Zero proof (guest `burn_proof`) that a burn note is in a
/// LEZ burn-accumulator root, and an `ILezAnchor` that says that root is final. `releaseOpen` (failsafe L2) checks
/// the same statement in plain SHA-256 for when the proof system is stopped; it reveals the owner's own link. In the sprint the anchor is
/// `StubLezAnchor`, an owner-posted stub (TRUST.md).
///
/// Caps and freeze are minimal stubs (F11, F14): per-token fixed-window caps, global and per-token freeze flags.
/// The verifier, the burn image id and the anchor are immutable (Security #8): changing them means a new vault.
contract Vault is ReentrancyGuard, AccessControl {
    using SafeERC20 for IERC20;

    // ------------------------------------------------------------------ constants

    /// @notice Native ETH is registered under this address.
    address public constant NATIVE = address(0);
    uint256 public constant TREE_DEPTH = 32;
    /// @notice Storage slot of the deposit root: keccak256("beacon-veil.vault.depositRoot") - 1. Fixed so the LEZ
    /// side can prove it with `eth_getProof` without depending on the compiler's layout.
    bytes32 public constant DEPOSIT_ROOT_SLOT = bytes32(uint256(keccak256("beacon-veil.vault.depositRoot")) - 1);
    bytes32 public constant FREEZER_ROLE = keccak256("FREEZER_ROLE");
    uint256 public constant JOURNAL_VERSION = 1;
    /// @notice Burn journal = 11 ABI words, see `BurnJournal`.
    uint256 public constant JOURNAL_LENGTH = 11 * 32;

    // ------------------------------------------------------------------ types

    /// @notice Public output of guest `burn_proof`, ABI-encoded as 11 static words.
    struct BurnJournal {
        uint256 version;
        uint256 chainId;
        address vault;
        bytes32 lezProgram;
        bytes32 lezInstance;
        bytes32 accumulatorRoot;
        bytes32 nullifier;
        address recipient;
        address token;
        uint256 denom;
        uint256 fee;
    }

    struct TokenConfig {
        bool registered; // deposits accepted
        bool releasable; // releases accepted (stays true after deregistration so holders can exit)
        bool depositFrozen;
        bool releaseFrozen;
        uint256[] denoms;
        uint256 depositCap; // per window, 0 = no deposits
        uint256 releaseCap; // per window, 0 = no releases
        uint256 windowStart;
        uint256 depositedInWindow;
        uint256 releasedInWindow;
    }

    /// @dev The 32 zero hashes, in code rather than storage (no cold SLOADs per deposit). Checked against
    /// `sha256` by `test_zeroHashesAreTheSha256Chain`.
    bytes private constant ZERO_HASHES =
        hex"0000000000000000000000000000000000000000000000000000000000000000f5a5fd42d16a20302798ef6ed309979b43003d2320d9f0e8ea9831a92759fb4bdb56114e00fdd4c1f85c892bf35ac9a89289aaecb1ebd0a96cde606a748b5d71c78009fdf07fc56a11f122370658a353aaa542ed63e44c4bc15ff4cd105ab33c536d98837f2dd165a55d5eeae91485954472d56f246df256bf3cae19352a123c9efde052aa15429fae05bad4d0b1d7c64da64d03d7a1854a588c2cb8430c0d30d88ddfeed400a8755596b21942c1497e114c302e6118290f91e6772976041fa187eb0ddba57e35f6d286673802a4af5975e22506c7cf4c64bb6be5ee11527f2c26846476fd5fc54a5d43385167c95144f2643f533cc85bb9d16b782f8d7db193506d86582d252405b840018792cad2bf1259f1ef5aa5f887e13cb2f0094f51e1ffff0ad7e659772f9534c195c815efc4014ef1e1daed4404c06385d11192e92b6cf04127db05441cd833107a52be852868890e4317e6a02ab47683aa75964220b7d05f875f140027ef5118a2247bbb84ce8f2f0f1123623085daf7960c329f5fdf6af5f5bbdb6be9ef8aa618e4bf8073960867171e29676f8b284dea6a08a85eb58d900f5e182e3c50ef74969ea16c7726c549757cc23523c369587da7293784d49a7502ffcfb0340b1d7885688500ca308161a7f96b62df9d083b71fcc8f2bb8fe6b1689256c0d385f42f5bbe2027a22c1996e110ba97c171d3e5948de92beb8d0d63c39ebade8509e0ae3c9c3876fb5fa112be18f905ecacfecb92057603ab95eec8b2e541cad4e91de38385f2e046619f54496c2382cb6cacd5b98c26f5a4f893e908917775b62bff23294dbbe3a1cd8e6cc1c35b4801887b646a6f81f17fcddba7b592e3133393c16194fac7431abf2f5485ed711db282183c819e08ebaa8a8d7fe3af8caa085a7639a832001457dfb9128a8061142ad0335629ff23ff9cfeb3c337d7a51a6fbf00b9e34c52e1c9195c969bd4e7a0bfd51d5c5bed9c1167e71f0aa83cc32edfbefa9f4d3e0174ca85182eec9f3a09f6a6c0df6377a510d731206fa80a50bb6abe29085058f16212212a60eec8f049fecb92d8c8e0a84bc021352bfecbeddde993839f614c3dac0a3ee37543f9b412b16199dc158e23b544619e312724bb6d7c3153ed9de791d764a366b389af13c58bf8a8d90481a467657cdd2986268250628d0c10e385c58c6191e6fbe05191bcc04f133f2cea72c1c4848930bd7ba8cac54661072113fb278869e07bb8587f91392933374d017bcbe18869ff2c22b28cc10510d9853292803328be4fb0e80495e8bb8d271f5b889636b5fe28e79f1b850f8658246ce9b6a1e7b49fc06db7143e8fe0b4f2b0c5523a5c985e929f70af28d0bdd1a90a808f977f597c7c778c489e98d3bd8910d31ac0f7";

    // ------------------------------------------------------------------ immutables

    /// @notice The RISC Zero Groth16 verifier, held through its `RiscZeroVerifierEmergencyStop` wrapper (failsafe
    /// L1). The wrapper can only stop verification, never change it (Security #8). Sepolia v3: estop
    /// 0xA1DCB16cb765b3CA71605339F0C37Bfe0F60bA29 around verifier 3.1.0 0x8c915b5071206C6E328BC18529586c513408aB3b.
    IRiscZeroVerifier public immutable verifier;
    ILezAnchor public immutable anchor;
    bytes32 public immutable burnImageId;
    /// @notice LEZ chain and zone this vault pairs with (F15), as an opaque 32-byte id.
    bytes32 public immutable lezZone;
    /// @notice Account id of the LEZ `bridge` program (F15).
    bytes32 public immutable lezProgram;

    // ------------------------------------------------------------------ state

    /// @notice Bridge instance id on LEZ. Written once by the admin after deployment, because the instance id is
    /// derived from a config that includes this vault's address. Zero until bound; releases need it.
    bytes32 public lezInstance;
    uint256 public capWindow;
    bool public globalDepositFrozen;
    bool public globalReleaseFrozen;
    /// @notice Per-route freeze (failsafe): the zero-knowledge route `release` and the open route `releaseOpen`.
    bool public zkRouteFrozen;
    bool public openRouteFrozen;

    uint256 public depositCount;
    bytes32[TREE_DEPTH] private branch;

    mapping(address => TokenConfig) private tokens;
    mapping(bytes32 => bool) public burnNullifierSpent;

    // ------------------------------------------------------------------ events and errors

    /// @notice One per deposit. Names no LEZ account (F2). `commitment` lets the owner's wallet find it again from
    /// the seed alone (F9).
    event Deposit(address indexed token, uint256 denom, bytes32 commitment, uint256 leafIndex, bytes32 root);
    event Release(address indexed token, uint256 denom, bytes32 nullifier, address recipient, uint256 fee);
    event TokenRegistered(address indexed token, uint256[] denoms, uint256 depositCap, uint256 releaseCap);
    event TokenDeregistered(address indexed token);
    event LezInstanceBound(bytes32 lezInstance);
    event FreezeChanged(address indexed token, bool deposits, bool releases);
    event RouteFreezeChanged(bool zkRoute, bool openRoute);

    error UnsupportedToken();
    error InvalidDenomination();
    error WrongValue();
    error FeeOnTransferNotSupported();
    error DepositsFrozen();
    error ReleasesFrozen();
    error CapExceeded();
    error TreeFull();
    error BadJournal();
    error WrongPairing();
    error RootNotFinal();
    error AlreadyReleased();
    error InstanceAlreadyBound();
    error InstanceNotBound();
    error EthTransferFailed();
    error RouteFrozen();

    constructor(
        IRiscZeroVerifier verifier_,
        ILezAnchor anchor_,
        bytes32 burnImageId_,
        bytes32 lezZone_,
        bytes32 lezProgram_,
        address admin,
        uint256 capWindow_
    ) {
        verifier = verifier_;
        anchor = anchor_;
        burnImageId = burnImageId_;
        lezZone = lezZone_;
        lezProgram = lezProgram_;
        capWindow = capWindow_;
        _grantRole(DEFAULT_ADMIN_ROLE, admin);
        _grantRole(FREEZER_ROLE, admin);
        _setDepositRoot(_computeRoot());
    }

    // ------------------------------------------------------------------ admin (stubs of F10, F11, F14)

    /// @notice Binds the LEZ bridge instance this vault releases for. Once only.
    function bindLezInstance(bytes32 instance) external onlyRole(DEFAULT_ADMIN_ROLE) {
        if (lezInstance != bytes32(0)) revert InstanceAlreadyBound();
        if (instance == bytes32(0)) revert WrongPairing();
        lezInstance = instance;
        emit LezInstanceBound(instance);
    }

    /// @notice Registers (or re-registers) a token with its permitted denominations and per-window caps.
    function registerToken(address token, uint256[] calldata denoms, uint256 depositCap, uint256 releaseCap)
        external
        onlyRole(DEFAULT_ADMIN_ROLE)
    {
        if (denoms.length == 0) revert InvalidDenomination();
        for (uint256 i = 0; i < denoms.length; i++) {
            if (denoms[i] == 0) revert InvalidDenomination();
        }
        TokenConfig storage t = tokens[token];
        t.registered = true;
        t.releasable = true;
        t.denoms = denoms;
        t.depositCap = depositCap;
        t.releaseCap = releaseCap;
        emit TokenRegistered(token, denoms, depositCap, releaseCap);
    }

    /// @notice Stops new deposits of `token`. Releases stay open so holders of the wrapped token can exit.
    function deregisterToken(address token) external onlyRole(DEFAULT_ADMIN_ROLE) {
        tokens[token].registered = false;
        emit TokenDeregistered(token);
    }

    function setCapWindow(uint256 seconds_) external onlyRole(DEFAULT_ADMIN_ROLE) {
        capWindow = seconds_;
    }

    /// @notice Freeze switches. `token == NATIVE` with `global == true` is not special: global flags are separate.
    function setGlobalFreeze(bool deposits, bool releases) external onlyRole(FREEZER_ROLE) {
        globalDepositFrozen = deposits;
        globalReleaseFrozen = releases;
        emit FreezeChanged(address(type(uint160).max), deposits, releases);
    }

    function setRouteFreeze(bool zkRoute, bool openRoute) external onlyRole(FREEZER_ROLE) {
        zkRouteFrozen = zkRoute;
        openRouteFrozen = openRoute;
        emit RouteFreezeChanged(zkRoute, openRoute);
    }

    function setTokenFreeze(address token, bool deposits, bool releases) external onlyRole(FREEZER_ROLE) {
        tokens[token].depositFrozen = deposits;
        tokens[token].releaseFrozen = releases;
        emit FreezeChanged(token, deposits, releases);
    }

    // ------------------------------------------------------------------ inbound

    /// @notice Escrows one denomination and appends its leaf. `commitment` is computed off chain from the wallet
    /// seed (docs/bridge/INBOUND.md); the vault never sees the destination.
    function deposit(address token, uint256 denomIndex, bytes32 commitment) external payable nonReentrant {
        TokenConfig storage t = tokens[token];
        if (!t.registered) revert UnsupportedToken();
        if (globalDepositFrozen || t.depositFrozen) revert DepositsFrozen();
        if (denomIndex >= t.denoms.length) revert InvalidDenomination();
        uint256 denom = t.denoms[denomIndex];
        _rollWindow(t);
        if (t.depositedInWindow + denom > t.depositCap) revert CapExceeded();
        t.depositedInWindow += denom;

        if (token == NATIVE) {
            if (msg.value != denom) revert WrongValue();
        } else {
            if (msg.value != 0) revert WrongValue();
            uint256 before = IERC20(token).balanceOf(address(this));
            IERC20(token).safeTransferFrom(msg.sender, address(this), denom);
            if (IERC20(token).balanceOf(address(this)) - before != denom) revert FeeOnTransferNotSupported();
        }

        bytes32 leaf = leafHash(token, denom, commitment);
        uint256 index = depositCount;
        _insert(leaf);
        bytes32 root = _computeRoot();
        _setDepositRoot(root);
        emit Deposit(token, denom, commitment, index, root);
    }

    /// @notice `sha256("wb/leaf/v1" || token (20 bytes) || denom (32 bytes, big-endian) || commitment)`.
    function leafHash(address token, uint256 denom, bytes32 commitment) public pure returns (bytes32) {
        return sha256(abi.encodePacked(bytes10("wb/leaf/v1"), token, denom, commitment));
    }

    /// @notice Current deposit root (also readable at `DEPOSIT_ROOT_SLOT`).
    function depositRoot() public view returns (bytes32 r) {
        bytes32 slot = DEPOSIT_ROOT_SLOT;
        assembly {
            r := sload(slot)
        }
    }

    // ------------------------------------------------------------------ outbound

    /// @notice Pays out one burn. `journal` is the 352-byte public output of guest `burn_proof`; `seal` its proof.
    /// The fee goes to the caller (a permissionless relayer); the recipient is bound in the proof.
    function release(bytes calldata seal, bytes calldata journal) external nonReentrant {
        if (zkRouteFrozen) revert RouteFrozen();
        if (journal.length != JOURNAL_LENGTH) revert BadJournal();
        BurnJournal memory j = abi.decode(journal, (BurnJournal));
        if (j.version != JOURNAL_VERSION) revert BadJournal();
        if (lezInstance == bytes32(0)) revert InstanceNotBound();
        if (j.chainId != block.chainid || j.vault != address(this) || j.lezProgram != lezProgram) {
            revert WrongPairing();
        }
        if (j.lezInstance != lezInstance) revert WrongPairing();
        _checkRelease(j.token, j.denom, j.fee, j.nullifier, j.accumulatorRoot);

        // Reverts on an invalid seal, or when the emergency stop has stopped the verifier.
        verifier.verify(seal, burnImageId, sha256(journal));

        _settle(j.token, j.denom, j.fee, j.nullifier, j.recipient);
    }

    /// @notice Failsafe L2 (docs/bridge/TRUST.md section 6): the open release route. Checks the same statement as guest
    /// `burn_proof` in plain SHA-256, so holders can exit if the proof system breaks. The owner reveals the burn secret,
    /// which links this release to its burn: opt-in. Shares the nullifier map with `release`, so a burn pays once.
    /// @param path Sibling hashes, leaf level first; `leafIndex` below 2^32.
    function releaseOpen(
        bytes32 burnSecret,
        address recipient,
        uint256 fee,
        address token,
        uint256 denom,
        uint256 leafIndex,
        bytes32[TREE_DEPTH] calldata path
    ) external nonReentrant {
        if (openRouteFrozen) revert RouteFrozen();
        (bytes32 root, bytes32 nullifier) = burnStatement(burnSecret, recipient, fee, token, denom, leafIndex, path);
        _checkRelease(token, denom, fee, nullifier, root);
        _settle(token, denom, fee, nullifier, recipient);
    }

    /// @notice The `burn_proof` statement: the accumulator root implied by the burn note's leaf and path, and the
    /// release nullifier. Same hashes as `programs/bridge/src/{hash,tree,journal}.rs`.
    function burnStatement(
        bytes32 burnSecret,
        address recipient,
        uint256 fee,
        address token,
        uint256 denom,
        uint256 leafIndex,
        bytes32[TREE_DEPTH] calldata path
    ) public pure returns (bytes32 root, bytes32 nullifier) {
        if (leafIndex >= (uint256(1) << TREE_DEPTH)) revert BadJournal();
        bytes32 note = sha256(abi.encodePacked(bytes7("wb/b/v1"), burnSecret, recipient, fee));
        root = sha256(abi.encodePacked(bytes11("wb/bleaf/v1"), token, denom, note));
        for (uint256 h = 0; h < TREE_DEPTH; h++) {
            root = ((leafIndex >> h) & 1) == 1
                ? sha256(abi.encodePacked(path[h], root))
                : sha256(abi.encodePacked(root, path[h]));
        }
        nullifier = sha256(abi.encodePacked(bytes8("wb/bn/v1"), burnSecret));
    }

    /// Checks shared by both release routes. The pairing (chain, vault, program, instance) is the anchor's: it
    /// accepts roots per instance, and the instance is this vault's bound one.
    function _checkRelease(address token, uint256 denom, uint256 fee, bytes32 nullifier, bytes32 root) private view {
        if (lezInstance == bytes32(0)) revert InstanceNotBound();
        TokenConfig storage t = tokens[token];
        if (!t.releasable) revert UnsupportedToken();
        if (globalReleaseFrozen || t.releaseFrozen) revert ReleasesFrozen();
        if (!_isDenom(t, denom) || fee >= denom) revert InvalidDenomination();
        if (burnNullifierSpent[nullifier]) revert AlreadyReleased();
        if (!anchor.isFinal(lezInstance, root)) revert RootNotFinal();
    }

    function _settle(address token, uint256 denom, uint256 fee, bytes32 nullifier, address recipient) private {
        TokenConfig storage t = tokens[token];
        _rollWindow(t);
        if (t.releasedInWindow + denom > t.releaseCap) revert CapExceeded();
        t.releasedInWindow += denom;
        burnNullifierSpent[nullifier] = true;
        _pay(token, recipient, denom - fee);
        if (fee != 0) _pay(token, msg.sender, fee);
        emit Release(token, denom, nullifier, recipient, fee);
    }

    // ------------------------------------------------------------------ views

    function tokenConfig(address token)
        external
        view
        returns (
            bool registered,
            bool releasable,
            bool depositFrozen,
            bool releaseFrozen,
            uint256[] memory denoms,
            uint256 depositCap,
            uint256 releaseCap
        )
    {
        TokenConfig storage t = tokens[token];
        return (t.registered, t.releasable, t.depositFrozen, t.releaseFrozen, t.denoms, t.depositCap, t.releaseCap);
    }

    /// @notice `zero[h]`: root of an empty subtree of height `h` (`zero[0] = 0`, `zero[h+1] = sha256(zero[h] || zero[h])`).
    function zeroHash(uint256 height) public pure returns (bytes32 z) {
        require(height < TREE_DEPTH, "height");
        bytes memory all = ZERO_HASHES;
        assembly {
            z := mload(add(all, mul(add(height, 1), 32)))
        }
    }

    // ------------------------------------------------------------------ internals

    function _isDenom(TokenConfig storage t, uint256 denom) private view returns (bool) {
        uint256 n = t.denoms.length;
        for (uint256 i = 0; i < n; i++) {
            if (t.denoms[i] == denom) return true;
        }
        return false;
    }

    function _rollWindow(TokenConfig storage t) private {
        if (block.timestamp >= t.windowStart + capWindow) {
            t.windowStart = block.timestamp;
            t.depositedInWindow = 0;
            t.releasedInWindow = 0;
        }
    }

    function _pay(address token, address to, uint256 amount) private {
        if (token == NATIVE) {
            (bool ok,) = payable(to).call{value: amount}("");
            if (!ok) revert EthTransferFailed();
        } else {
            IERC20(token).safeTransfer(to, amount);
        }
    }

    /// Deposit-contract insert: walk up while the index bit is 1.
    function _insert(bytes32 leaf) private {
        uint256 count = depositCount;
        if (count >= (uint256(1) << TREE_DEPTH) - 1) revert TreeFull();
        count += 1;
        depositCount = count;
        bytes32 node = leaf;
        uint256 size = count;
        for (uint256 h = 0; h < TREE_DEPTH; h++) {
            if ((size & 1) == 1) {
                branch[h] = node;
                return;
            }
            node = sha256(abi.encodePacked(branch[h], node));
            size /= 2;
        }
    }

    /// Deposit-contract root, without the length mix-in: a plain depth-32 binary Merkle root, empty leaves zero.
    function _computeRoot() private view returns (bytes32 node) {
        uint256 size = depositCount;
        bytes memory zeros = ZERO_HASHES;
        for (uint256 h = 0; h < TREE_DEPTH; h++) {
            if ((size & 1) == 1) {
                node = sha256(abi.encodePacked(branch[h], node));
            } else {
                bytes32 z;
                assembly {
                    z := mload(add(zeros, mul(add(h, 1), 32)))
                }
                node = sha256(abi.encodePacked(node, z));
            }
            size /= 2;
        }
    }

    function _setDepositRoot(bytes32 root) private {
        bytes32 slot = DEPOSIT_ROOT_SLOT;
        assembly {
            sstore(slot, root)
        }
    }

    receive() external payable {
        revert WrongValue();
    }
}
