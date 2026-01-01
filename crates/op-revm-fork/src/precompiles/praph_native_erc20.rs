//! Native PRAF ERC-20 precompile implementation for op-revm-praph
//!
//! This module implements a stateful precompile that exposes the native gas token (PRAF)
//! as an ERC-20 token at address 0x805.

extern crate alloc;

use alloy_primitives::{address, Address, Bytes, FixedBytes, U256};
use alloy_sol_types::{sol, SolEvent, SolInterface, SolValue};
use revm::{
    context_interface::{ContextTr, JournalTr},
    interpreter::{CallInputs, InstructionResult, InterpreterResult, Gas},
    primitives::{Log, LogData},
};

/// Native PRAF token address (0x805)
pub const NATIVE_TOKEN_ADDRESS: Address = address!("0000000000000000000000000000000000000805");

sol! {
    interface IERC20 {
        function name() external view returns (string);
        function symbol() external view returns (string);
        function decimals() external view returns (uint8);
        function totalSupply() external view returns (uint256);
        function balanceOf(address account) external view returns (uint256);
        function transfer(address recipient, uint256 amount) external returns (bool);
        function allowance(address owner, address spender) external view returns (uint256);
        function approve(address spender, uint256 amount) external returns (bool);
        function transferFrom(address sender, address recipient, uint256 amount) external returns (bool);
        function mint(address to, uint256 amount) external;
        function setMinter(address minter) external;
    }

    event Transfer(address indexed from, address indexed to, uint256 value);
    event Approval(address indexed owner, address indexed spender, uint256 value);
}

const ALLOWANCE_SLOT: U256 = U256::ZERO;
const MINTER_SLOT: U256 = U256::from_limbs([1, 0, 0, 0]);
const ADMIN_ADDRESS: Address = address!("f39Fd6e51aad88F6F4ce6aB8827279cffFb92266");

/// Precompile entrypoint called from OpPrecompiles::run()
pub fn run_native_praf<CTX>(
    context: &mut CTX,
    inputs: &CallInputs,
) -> Option<InterpreterResult>
where
    CTX: ContextTr,
{
    eprintln!("[DEBUG] Native PRAF 0x805 CALLED! target={:?}", inputs.target_address);
    let caller = inputs.caller;
    let is_static = inputs.is_static;
    let input_bytes = inputs.input.bytes(context);
    eprintln!("[DEBUG] Caller: {:?}, Input length: {}, IsStatic: {}", caller, input_bytes.len(), is_static);
    precompile_run(context, caller, &input_bytes, is_static)
}

/// Core precompile logic
fn precompile_run<CTX>(
    context: &mut CTX,
    caller: Address,
    input: &Bytes,
    is_static: bool,
) -> Option<InterpreterResult>
where
    CTX: ContextTr,
{
    let call = IERC20::IERC20Calls::abi_decode(input).ok()?;

    match call {
        IERC20::IERC20Calls::name(_) => {
            Some(InterpreterResult {
                result: InstructionResult::Return,
                output: ("PRAPH".to_string(),).abi_encode().into(),
                gas: Gas::new(0),
            })
        }
        IERC20::IERC20Calls::symbol(_) => {
            Some(InterpreterResult {
                result: InstructionResult::Return,
                output: ("PRAF".to_string(),).abi_encode().into(),
                gas: Gas::new(0),
            })
        }
        IERC20::IERC20Calls::decimals(_) => {
            Some(InterpreterResult {
                result: InstructionResult::Return,
                output: (U256::from(18),).abi_encode().into(),
                gas: Gas::new(0),
            })
        }
        IERC20::IERC20Calls::totalSupply(_) => {
            Some(InterpreterResult {
                result: InstructionResult::Return,
                output: (U256::MAX,).abi_encode().into(),
                gas: Gas::new(0),
            })
        }
        IERC20::IERC20Calls::balanceOf(args) => {
            let load = context.balance(args.account)?;
            Some(InterpreterResult {
                result: InstructionResult::Return,
                output: (load.data,).abi_encode().into(),
                gas: Gas::new(0),
            })
        }
        IERC20::IERC20Calls::transfer(args) => {
            if is_static {
                return Some(InterpreterResult {
                    result: InstructionResult::Revert,
                    output: Bytes::new(),
                    gas: Gas::new(0),
                });
            }

            match context.journal_mut().transfer(caller, args.recipient, args.amount) {
                Ok(None) => {
                    context.journal_mut().log(Log {
                        address: NATIVE_TOKEN_ADDRESS,
                        data: LogData::new_unchecked(
                            alloc::vec![
                                Transfer::SIGNATURE_HASH,
                                FixedBytes::from(caller.into_word()),
                                FixedBytes::from(args.recipient.into_word()),
                            ],
                            (args.amount,).abi_encode().into(),
                        ),
                    });
                    Some(InterpreterResult {
                        result: InstructionResult::Return,
                        output: (true,).abi_encode().into(),
                        gas: Gas::new(0),
                    })
                }
                _ => Some(InterpreterResult {
                    result: InstructionResult::Revert,
                    output: Bytes::from("Transfer failed"),
                    gas: Gas::new(0),
                }),
            }
        }
        IERC20::IERC20Calls::allowance(args) => {
            let slot = get_map_slot(get_map_slot(ALLOWANCE_SLOT, args.owner), args.spender);
            let load = context.sload(NATIVE_TOKEN_ADDRESS, slot)?;
            Some(InterpreterResult {
                result: InstructionResult::Return,
                output: (load.data,).abi_encode().into(),
                gas: Gas::new(0),
            })
        }
        IERC20::IERC20Calls::approve(args) => {
            if is_static {
                return Some(InterpreterResult {
                    result: InstructionResult::Revert,
                    output: Bytes::new(),
                    gas: Gas::new(0),
                });
            }

            let slot = get_map_slot(get_map_slot(ALLOWANCE_SLOT, caller), args.spender);
            context.sstore(NATIVE_TOKEN_ADDRESS, slot, args.amount)?;
            
            context.journal_mut().log(Log {
                address: NATIVE_TOKEN_ADDRESS,
                data: LogData::new_unchecked(
                    alloc::vec![
                        Approval::SIGNATURE_HASH,
                        FixedBytes::from(caller.into_word()),
                        FixedBytes::from(args.spender.into_word()),
                    ],
                    (args.amount,).abi_encode().into(),
                ),
            });
            
            Some(InterpreterResult {
                result: InstructionResult::Return,
                output: (true,).abi_encode().into(),
                gas: Gas::new(0),
            })
        }
        IERC20::IERC20Calls::transferFrom(args) => {
            if is_static {
                return Some(InterpreterResult {
                    result: InstructionResult::Revert,
                    output: Bytes::new(),
                    gas: Gas::new(0),
                });
            }

            let slot = get_map_slot(get_map_slot(ALLOWANCE_SLOT, args.sender), caller);
            let load = context.sload(NATIVE_TOKEN_ADDRESS, slot)?;
            let allowance = load.data;

            if allowance < args.amount {
                return Some(InterpreterResult {
                    result: InstructionResult::Revert,
                    output: Bytes::from("Insufficient allowance"),
                    gas: Gas::new(0),
                });
            }

            if allowance != U256::MAX {
                context.sstore(NATIVE_TOKEN_ADDRESS, slot, allowance - args.amount)?;
            }

            match context.journal_mut().transfer(args.sender, args.recipient, args.amount) {
                Ok(None) => {
                    context.journal_mut().log(Log {
                        address: NATIVE_TOKEN_ADDRESS,
                        data: LogData::new_unchecked(
                            alloc::vec![
                                Transfer::SIGNATURE_HASH,
                                FixedBytes::from(args.sender.into_word()),
                                FixedBytes::from(args.recipient.into_word()),
                            ],
                            (args.amount,).abi_encode().into(),
                        ),
                    });
                    Some(InterpreterResult {
                        result: InstructionResult::Return,
                        output: (true,).abi_encode().into(),
                        gas: Gas::new(0),
                    })
                }
                _ => Some(InterpreterResult {
                    result: InstructionResult::Revert,
                    output: Bytes::from("Transfer failed"),
                    gas: Gas::new(0),
                }),
            }
        }
        IERC20::IERC20Calls::mint(args) => {
            if is_static {
                return Some(InterpreterResult {
                    result: InstructionResult::Revert,
                    output: Bytes::new(),
                    gas: Gas::new(0),
                });
            }

            let load = context.sload(NATIVE_TOKEN_ADDRESS, MINTER_SLOT)?;
            let minter = Address::from_word(load.data.into());

            if caller != minter && caller != ADMIN_ADDRESS {
                return Some(InterpreterResult {
                    result: InstructionResult::Revert,
                    output: Bytes::from("Only minter or admin"),
                    gas: Gas::new(0),
                });
            }

            match context.journal_mut().transfer(Address::ZERO, args.to, args.amount) {
                Ok(_) => {
                    context.journal_mut().log(Log {
                        address: NATIVE_TOKEN_ADDRESS,
                        data: LogData::new_unchecked(
                            alloc::vec![
                                Transfer::SIGNATURE_HASH,
                                FixedBytes::from(Address::ZERO.into_word()),
                                FixedBytes::from(args.to.into_word()),
                            ],
                            (args.amount,).abi_encode().into(),
                        ),
                    });
                    Some(InterpreterResult {
                        result: InstructionResult::Return,
                        output: Bytes::new(),
                        gas: Gas::new(0),
                    })
                }
                Err(_) => Some(InterpreterResult {
                    result: InstructionResult::Revert,
                    output: Bytes::from("Mint failed"),
                    gas: Gas::new(0),
                }),
            }
        }
        IERC20::IERC20Calls::setMinter(args) => {
            if is_static {
                return Some(InterpreterResult {
                    result: InstructionResult::Revert,
                    output: Bytes::new(),
                    gas: Gas::new(0),
                });
            }

            if caller != ADMIN_ADDRESS {
                return Some(InterpreterResult {
                    result: InstructionResult::Revert,
                    output: Bytes::from("Only admin"),
                    gas: Gas::new(0),
                });
            }

            context.sstore(
                NATIVE_TOKEN_ADDRESS,
                MINTER_SLOT,
                U256::from_be_bytes(args.minter.into_word().0),
            )?;

            Some(InterpreterResult {
                result: InstructionResult::Return,
                output: Bytes::new(),
                gas: Gas::new(0),
            })
        }
    }
}

fn get_map_slot(map_slot: U256, key: Address) -> U256 {
    use alloy_primitives::Keccak256;
    let mut hasher = Keccak256::new();
    hasher.update(key.into_word());
    hasher.update(map_slot.to_be_bytes::<32>());
    hasher.finalize().into()
}

// ===== Static Precompile for Precompiles Map =====

use revm::precompile::{Precompile, PrecompileId, PrecompileResult, PrecompileOutput};

/// Static precompile constant for native PRAF ERC-20
pub const NATIVE_PRAF_PRECOMPILE: Precompile = Precompile::new(
    PrecompileId::Custom(std::borrow::Cow::Borrowed("praph_native_praf")),
    NATIVE_TOKEN_ADDRESS,
    run_native_praf_static,
);

/// Static precompile function matching revm's signature
fn run_native_praf_static(input: &[u8], _gas_limit: u64) -> PrecompileResult {
    use alloc::string::ToString;
    
    eprintln!("[DEBUG STATIC] Native PRAF static precompile called! input_len={}", input.len());
    
    if input.len() >= 4 {
        let selector = &input[0..4];
        eprintln!("[DEBUG STATIC] Selector: {:02x?}", selector);
        
        match selector {
            [0x95, 0xd8, 0x9b, 0x41] => {
                let result = ("PRAF".to_string(),).abi_encode();
                eprintln!("[DEBUG STATIC] Returning symbol: PRAF");
                return Ok(PrecompileOutput::new(0, result.into()));
            }
            [0x06, 0xfd, 0xde, 0x03] => {
                let result = ("PRAPH".to_string(),).abi_encode();
                eprintln!("[DEBUG STATIC] Returning name: PRAPH");
                return Ok(PrecompileOutput::new(0, result.into()));
            }
            [0x31, 0x3c, 0xe5, 0x67] => {
                let result = (U256::from(18),).abi_encode();
                eprintln!("[DEBUG STATIC] Returning decimals: 18");
                return Ok(PrecompileOutput::new(0, result.into()));
            }
            _ => {
                eprintln!("[DEBUG STATIC] Unknown selector");
            }
        }
    }
    
    Ok(PrecompileOutput::new(0, Bytes::new()))
}
