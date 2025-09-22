pragma solidity ^0.8.28;

// CoreWriter Interface
interface ICoreWriter {
    function sendRawAction(bytes calldata data) external;
}