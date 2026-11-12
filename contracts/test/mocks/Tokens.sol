// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {ERC20} from "@openzeppelin/contracts/token/ERC20/ERC20.sol";

/// @notice Plain 6-decimal ERC-20 (USDC-like).
contract MockToken is ERC20 {
    uint8 private immutable dec;

    constructor(string memory name_, uint8 decimals_) ERC20(name_, name_) {
        dec = decimals_;
    }

    function decimals() public view override returns (uint8) {
        return dec;
    }

    function mint(address to, uint256 amount) external {
        _mint(to, amount);
    }
}

/// @notice USDT-style token: `transfer`, `transferFrom` and `approve` return nothing (F17).
/// Written from the shape of Tether's mainnet TetherToken ABI, not copied.
contract MockUsdt {
    string public constant name = "Mock USDT";
    string public constant symbol = "mUSDT";
    uint8 public constant decimals = 6;
    uint256 public totalSupply;
    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;

    function mint(address to, uint256 amount) external {
        balanceOf[to] += amount;
        totalSupply += amount;
    }

    function transfer(address to, uint256 amount) external {
        require(balanceOf[msg.sender] >= amount, "balance");
        balanceOf[msg.sender] -= amount;
        balanceOf[to] += amount;
    }

    function transferFrom(address from, address to, uint256 amount) external {
        require(balanceOf[from] >= amount, "balance");
        require(allowance[from][msg.sender] >= amount, "allowance");
        allowance[from][msg.sender] -= amount;
        balanceOf[from] -= amount;
        balanceOf[to] += amount;
    }

    function approve(address spender, uint256 amount) external {
        // Tether refuses changing a non-zero allowance to another non-zero value.
        require(amount == 0 || allowance[msg.sender][spender] == 0, "approve");
        allowance[msg.sender][spender] = amount;
    }
}

/// @notice Takes 1% on every transfer. The vault must refuse it.
contract MockFeeToken is ERC20 {
    constructor() ERC20("Fee", "FEE") {}

    function mint(address to, uint256 amount) external {
        _mint(to, amount);
    }

    function _update(address from, address to, uint256 value) internal override {
        if (from != address(0) && to != address(0)) {
            uint256 fee = value / 100;
            super._update(from, address(0xdead), fee);
            super._update(from, to, value - fee);
        } else {
            super._update(from, to, value);
        }
    }
}

/// @notice Recipient that rejects ETH.
contract RejectEth {
    receive() external payable {
        revert("no");
    }
}
