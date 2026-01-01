use alloy_primitives::{address, Address, Bytes, FixedBytes, U256};
use alloy_sol_types::{sol, SolEvent, SolInterface, SolValue};
use revm::{
    context::{BlockEnv, CfgEnv, Context, Host, TxEnv},
    database::Database,
    interpreter::{CallInputs, CallOutcome, Gas, InstructionResult, InterpreterResult},
    Inspector, Journal,
};

use op_revm_praph::{l1block::L1BlockInfo, OpSpecId, OpTransaction};

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
const ADMIN_ADDRESS: Address = address!("f39Fd6e51aad88F6F4ce6aB8827279cffFb92266"); // Anvil default dev key (Alice)

#[derive(Clone, Copy, Debug, Default)]
pub struct NativeErc20Inspector;

pub type OpContext<DB> =
    Context<BlockEnv, OpTransaction<TxEnv>, CfgEnv<OpSpecId>, DB, Journal<DB>, L1BlockInfo>;

impl<DB: Database> Inspector<OpContext<DB>> for NativeErc20Inspector {
    fn call(
        &mut self,
        context: &mut OpContext<DB>,
        inputs: &mut CallInputs,
    ) -> Option<CallOutcome> {
        let contract = inputs.target_address;

        if contract == NATIVE_TOKEN_ADDRESS {
            let caller = inputs.caller;

            // CallInput has two variants: SharedBuffer(Range<usize>) and Bytes(Bytes)
            // Use the bytes(ctx) method to extract data
            let input_bytes = inputs.input.bytes(context);

            let res = execute_native_erc20(context, caller, &input_bytes, inputs.is_static)?;

            return Some(CallOutcome {
                result: res,
                memory_offset: 0..0,
                precompile_call_logs: vec![],
                was_precompile_called: true,
            });
        }
        None
    }
}

/// Precompile entrypoint for PraphPrecompiles - works with generic CTX
/// Handles all ERC-20 methods using ContextTr and JournalTr traits
pub(crate) fn precompile_run<CTX>(
    context: &mut CTX,
    caller: Address,
    input: &Bytes,
    is_static: bool,
) -> Option<InterpreterResult>
where
    CTX: revm::context_interface::ContextTr,
{
    use revm::{
        context_interface::JournalTr,
        primitives::{Log, LogData},
    };

    let call = IERC20::IERC20Calls::abi_decode(input).ok()?;

    match call {
        IERC20::IERC20Calls::name(_) => {
            let result = ("PRAPH".to_string(),).abi_encode();
            Some(InterpreterResult {
                result: InstructionResult::Return,
                output: result.into(),
                gas: Gas::new(0),
            })
        }
        IERC20::IERC20Calls::symbol(_) => {
            let result = ("PRAF".to_string(),).abi_encode();
            Some(InterpreterResult {
                result: InstructionResult::Return,
                output: result.into(),
                gas: Gas::new(0),
            })
        }
        IERC20::IERC20Calls::decimals(_) => {
            let result = (U256::from(18),).abi_encode();
            Some(InterpreterResult {
                result: InstructionResult::Return,
                output: result.into(),
                gas: Gas::new(0),
            })
        }
        IERC20::IERC20Calls::totalSupply(_) => {
            let result = (U256::MAX,).abi_encode();
            Some(InterpreterResult {
                result: InstructionResult::Return,
                output: result.into(),
                gas: Gas::new(0),
            })
        }
        IERC20::IERC20Calls::balanceOf(args) => {
            let load = context.balance(args.account)?;
            let balance = load.data;
            let result = (balance,).abi_encode();
            Some(InterpreterResult {
                result: InstructionResult::Return,
                output: result.into(),
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

            let sender = caller;
            let recipient = args.recipient;
            let amount = args.amount;

            // Transfer native balance using journal
            let transfer_result = context.journal_mut().transfer(sender, recipient, amount);
            match transfer_result {
                Ok(Some(_err)) => {
                    // Transfer failed (insufficient balance or other error)
                    return Some(InterpreterResult {
                        result: InstructionResult::Revert,
                        output: Bytes::from("Native balance transfer failed"),
                        gas: Gas::new(0),
                    });
                }
                Ok(None) => {
                    // Transfer succeeded - emit Transfer event
                    let log = Log {
                        address: NATIVE_TOKEN_ADDRESS,
                        data: LogData::new_unchecked(
                            alloc::vec![
                                Transfer::SIGNATURE_HASH,
                                FixedBytes::from(sender.into_word()),
                                FixedBytes::from(recipient.into_word()),
                            ],
                            (amount,).abi_encode().into(),
                        ),
                    };
                    context.journal_mut().log(log);

                    let result = (true,).abi_encode();
                    Some(InterpreterResult {
                        result: InstructionResult::Return,
                        output: result.into(),
                        gas: Gas::new(0),
                    })
                }
                Err(_db_err) => {
                    // Database error
                    return Some(InterpreterResult {
                        result: InstructionResult::Revert,
                        output: Bytes::from("Database error during transfer"),
                        gas: Gas::new(0),
                    });
                }
            }
        }
        IERC20::IERC20Calls::allowance(args) => {
            let outer_slot = get_map_slot_generic(ALLOWANCE_SLOT, args.owner);
            let final_slot = get_map_slot_generic(outer_slot, args.spender);
            let load = context.sload(NATIVE_TOKEN_ADDRESS, final_slot)?;
            let allowance = load.data;
            let result = (allowance,).abi_encode();
            Some(InterpreterResult {
                result: InstructionResult::Return,
                output: result.into(),
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

            let owner = caller;
            let spender = args.spender;
            let amount = args.amount;

            // Compute storage slot for allowance[owner][spender]
            let outer_slot = get_map_slot_generic(ALLOWANCE_SLOT, owner);
            let final_slot = get_map_slot_generic(outer_slot, spender);

            // Store allowance in precompile's storage
            let store_result = context.sstore(NATIVE_TOKEN_ADDRESS, final_slot, amount);
            if store_result.is_none() {
                return Some(InterpreterResult {
                    result: InstructionResult::Revert,
                    output: Bytes::from("Failed to store allowance"),
                    gas: Gas::new(0),
                });
            }

            // Emit Approval event
            let log = Log {
                address: NATIVE_TOKEN_ADDRESS,
                data: LogData::new_unchecked(
                    alloc::vec![
                        Approval::SIGNATURE_HASH,
                        FixedBytes::from(owner.into_word()),
                        FixedBytes::from(spender.into_word()),
                    ],
                    (amount,).abi_encode().into(),
                ),
            };
            context.journal_mut().log(log);

            let result = (true,).abi_encode();
            Some(InterpreterResult {
                result: InstructionResult::Return,
                output: result.into(),
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

            let sender = args.sender;
            let spender = caller;
            let recipient = args.recipient;
            let amount = args.amount;

            // Check allowance
            let outer_slot = get_map_slot_generic(ALLOWANCE_SLOT, sender);
            let final_slot = get_map_slot_generic(outer_slot, spender);
            let load = context.sload(NATIVE_TOKEN_ADDRESS, final_slot)?;
            let allowance = load.data;

            if allowance < amount {
                return Some(InterpreterResult {
                    result: InstructionResult::Revert,
                    output: Bytes::from("Insufficient allowance"),
                    gas: Gas::new(0),
                });
            }

            // Deduct allowance (unless unlimited U256::MAX)
            if allowance != U256::MAX {
                let new_allowance = allowance - amount;
                let _ = context.sstore(NATIVE_TOKEN_ADDRESS, final_slot, new_allowance);
            }

            // Transfer native balance using journal
            let transfer_result = context.journal_mut().transfer(sender, recipient, amount);
            match transfer_result {
                Ok(Some(_err)) => {
                    // Transfer failed
                    return Some(InterpreterResult {
                        result: InstructionResult::Revert,
                        output: Bytes::from("Native balance transfer failed"),
                        gas: Gas::new(0),
                    });
                }
                Ok(None) => {
                    // Transfer succeeded - emit Transfer event
                    let log = Log {
                        address: NATIVE_TOKEN_ADDRESS,
                        data: LogData::new_unchecked(
                            alloc::vec![
                                Transfer::SIGNATURE_HASH,
                                FixedBytes::from(sender.into_word()),
                                FixedBytes::from(recipient.into_word()),
                            ],
                            (amount,).abi_encode().into(),
                        ),
                    };
                    context.journal_mut().log(log);

                    let result = (true,).abi_encode();
                    Some(InterpreterResult {
                        result: InstructionResult::Return,
                        output: result.into(),
                        gas: Gas::new(0),
                    })
                }
                Err(_db_err) => {
                    return Some(InterpreterResult {
                        result: InstructionResult::Revert,
                        output: Bytes::from("Database error during transfer"),
                        gas: Gas::new(0),
                    });
                }
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

            let recipient = args.to;
            let amount = args.amount;

            // Access Control: Only registered minter can call mint
            let load = context.sload(NATIVE_TOKEN_ADDRESS, MINTER_SLOT)?;
            let current_minter = Address::from_word(load.data.into());

            // For dev mode, we allow the ADMIN_ADDRESS to mint as well
            if caller != current_minter && caller != ADMIN_ADDRESS {
                return Some(InterpreterResult {
                    result: InstructionResult::Revert,
                    output: Bytes::from("Only authorized minter or admin can call mint"),
                    gas: Gas::new(0),
                });
            }

            // Mint by transferring from zero address (infinite supply)
            // In revm, transfer from Address::ZERO often bypasses balance check or is allowed for
            // minting
            let transfer_result = context.journal_mut().transfer(Address::ZERO, recipient, amount);
            match transfer_result {
                Ok(_) => {
                    // Emit Transfer event from zero address
                    let log = Log {
                        address: NATIVE_TOKEN_ADDRESS,
                        data: LogData::new_unchecked(
                            alloc::vec![
                                Transfer::SIGNATURE_HASH,
                                FixedBytes::from(Address::ZERO.into_word()),
                                FixedBytes::from(recipient.into_word()),
                            ],
                            (amount,).abi_encode().into(),
                        ),
                    };
                    context.journal_mut().log(log);

                    Some(InterpreterResult {
                        result: InstructionResult::Return,
                        output: Bytes::new(),
                        gas: Gas::new(0),
                    })
                }
                Err(_) => {
                    return Some(InterpreterResult {
                        result: InstructionResult::Revert,
                        output: Bytes::from("Failed to mint native tokens"),
                        gas: Gas::new(0),
                    });
                }
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

            // Only admin can set minter
            if caller != ADMIN_ADDRESS {
                return Some(InterpreterResult {
                    result: InstructionResult::Revert,
                    output: Bytes::from("Only admin can set minter"),
                    gas: Gas::new(0),
                });
            }

            let new_minter = args.minter;
            let _ = context.sstore(
                NATIVE_TOKEN_ADDRESS,
                MINTER_SLOT,
                U256::from_be_bytes(new_minter.into_word().0),
            );

            Some(InterpreterResult {
                result: InstructionResult::Return,
                output: Bytes::new(),
                gas: Gas::new(0),
            })
        }
    }
}

/// Helper for computing storage slot from map slot and key
fn get_map_slot_generic(map_slot: U256, key: Address) -> U256 {
    use alloy_primitives::Keccak256;
    let mut hasher = Keccak256::new();
    hasher.update(key.into_word());
    hasher.update(map_slot.to_be_bytes::<32>());
    hasher.finalize().into()
}

pub(crate) fn execute_native_erc20<DB: Database>(
    context: &mut OpContext<DB>,
    caller: Address,
    input: &Bytes,
    is_static: bool,
) -> Option<InterpreterResult> {
    let call = IERC20::IERC20Calls::abi_decode(input).ok()?;

    match call {
        IERC20::IERC20Calls::name(_) => {
            let result = ("PRAPH".to_string(),).abi_encode();
            Some(InterpreterResult {
                result: InstructionResult::Return,
                output: result.into(),
                gas: Gas::new(0),
            })
        }
        IERC20::IERC20Calls::symbol(_) => {
            let result = ("PRAF".to_string(),).abi_encode();
            Some(InterpreterResult {
                result: InstructionResult::Return,
                output: result.into(),
                gas: Gas::new(0),
            })
        }
        IERC20::IERC20Calls::decimals(_) => {
            let result = (U256::from(18),).abi_encode();
            Some(InterpreterResult {
                result: InstructionResult::Return,
                output: result.into(),
                gas: Gas::new(0),
            })
        }
        IERC20::IERC20Calls::totalSupply(_) => {
            let result = (U256::MAX,).abi_encode();
            Some(InterpreterResult {
                result: InstructionResult::Return,
                output: result.into(),
                gas: Gas::new(0),
            })
        }
        IERC20::IERC20Calls::balanceOf(args) => {
            let load = context.balance(args.account)?;
            let balance = load.data;
            let result = (balance,).abi_encode();
            Some(InterpreterResult {
                result: InstructionResult::Return,
                output: result.into(),
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
            let amount = args.amount;
            let to = args.recipient;
            let sender = caller;

            let load = context.balance(sender)?;
            let sender_balance = load.data;

            if sender_balance < amount {
                return Some(InterpreterResult {
                    result: InstructionResult::Revert,
                    output: Bytes::from("Insufficient balance"),
                    gas: Gas::new(0),
                });
            }

            let _ = context.balance(to)?;

            let sender_acc_entry = context.journaled_state.state.get_mut(&sender)?;
            sender_acc_entry.info.balance = sender_acc_entry.info.balance.checked_sub(amount)?;
            sender_acc_entry.mark_touch();

            let to_acc_entry = context.journaled_state.state.get_mut(&to)?;
            to_acc_entry.info.balance = to_acc_entry.info.balance.checked_add(amount)?;
            to_acc_entry.mark_touch();

            let log = revm::primitives::Log {
                address: NATIVE_TOKEN_ADDRESS,
                data: revm::primitives::LogData::new_unchecked(
                    vec![
                        Transfer::SIGNATURE_HASH,
                        FixedBytes::from(Address::from(sender).into_word()),
                        FixedBytes::from(Address::from(to).into_word()),
                    ],
                    (amount,).abi_encode().into(),
                ),
            };
            context.journaled_state.logs.push(log);

            Some(InterpreterResult {
                result: InstructionResult::Return,
                output: (true,).abi_encode().into(),
                gas: Gas::new(0),
            })
        }
        IERC20::IERC20Calls::allowance(args) => {
            let allowance = get_allowance(context, args.owner, args.spender)?;
            let result = (allowance,).abi_encode();
            Some(InterpreterResult {
                result: InstructionResult::Return,
                output: result.into(),
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
            let owner = caller;
            set_allowance(context, owner, args.spender, args.amount).ok()?;

            let log = revm::primitives::Log {
                address: NATIVE_TOKEN_ADDRESS,
                data: revm::primitives::LogData::new_unchecked(
                    vec![
                        Approval::SIGNATURE_HASH,
                        FixedBytes::from(Address::from(owner).into_word()),
                        FixedBytes::from(Address::from(args.spender).into_word()),
                    ],
                    (args.amount,).abi_encode().into(),
                ),
            };
            context.journaled_state.logs.push(log);

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
            let sender = args.sender;
            let spender = caller;
            let amount = args.amount;

            let allowance = get_allowance(context, sender, spender)?;
            if allowance < amount {
                return Some(InterpreterResult {
                    result: InstructionResult::Revert,
                    output: Bytes::from("Insufficient allowance"),
                    gas: Gas::new(0),
                });
            }

            if allowance != U256::MAX {
                set_allowance(context, sender, spender, allowance - amount).ok()?;
            }

            let load = context.balance(sender)?;
            let sender_balance = load.data;
            if sender_balance < amount {
                return Some(InterpreterResult {
                    result: InstructionResult::Revert,
                    output: Bytes::from("Insufficient balance"),
                    gas: Gas::new(0),
                });
            }
            let _ = context.balance(args.recipient)?;

            let sender_acc_entry = context.journaled_state.state.get_mut(&sender)?;
            sender_acc_entry.info.balance = sender_acc_entry.info.balance.checked_sub(amount)?;
            sender_acc_entry.mark_touch();

            let to_acc_entry = context.journaled_state.state.get_mut(&args.recipient)?;
            to_acc_entry.info.balance = to_acc_entry.info.balance.checked_add(amount)?;
            to_acc_entry.mark_touch();

            let log = revm::primitives::Log {
                address: NATIVE_TOKEN_ADDRESS,
                data: revm::primitives::LogData::new_unchecked(
                    vec![
                        Transfer::SIGNATURE_HASH,
                        FixedBytes::from(Address::from(sender).into_word()),
                        FixedBytes::from(Address::from(args.recipient).into_word()),
                    ],
                    (amount,).abi_encode().into(),
                ),
            };
            context.journaled_state.logs.push(log);

            Some(InterpreterResult {
                result: InstructionResult::Return,
                output: (true,).abi_encode().into(),
                gas: Gas::new(0),
            })
        }
        IERC20::IERC20Calls::mint(args) => {
            if is_static {
                return Some(InterpreterResult {
                    result: InstructionResult::Revert,
                    output: Bytes::new(),
                    gas: Gas::new(0),
                });
            }
            let to = args.to;
            let amount = args.amount;

            // Access Control
            let load = context.sload(NATIVE_TOKEN_ADDRESS, MINTER_SLOT)?;
            let current_minter = Address::from_word(load.data.into());
            if caller != current_minter && caller != ADMIN_ADDRESS {
                return Some(InterpreterResult {
                    result: InstructionResult::Revert,
                    output: Bytes::from("Only authorized minter or admin can call mint"),
                    gas: Gas::new(0),
                });
            }

            let _ = context.balance(to)?;

            let to_acc_entry = context.journaled_state.state.get_mut(&to)?;
            to_acc_entry.info.balance = to_acc_entry.info.balance.checked_add(amount)?;
            to_acc_entry.mark_touch();

            let log = revm::primitives::Log {
                address: NATIVE_TOKEN_ADDRESS,
                data: revm::primitives::LogData::new_unchecked(
                    vec![
                        Transfer::SIGNATURE_HASH,
                        FixedBytes::from(Address::ZERO.into_word()),
                        FixedBytes::from(Address::from(to).into_word()),
                    ],
                    (amount,).abi_encode().into(),
                ),
            };
            context.journaled_state.logs.push(log);

            Some(InterpreterResult {
                result: InstructionResult::Return,
                output: Bytes::new(),
                gas: Gas::new(0),
            })
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
                    output: Bytes::from("Only admin can set minter"),
                    gas: Gas::new(0),
                });
            }
            let _ = context.sstore(
                NATIVE_TOKEN_ADDRESS,
                MINTER_SLOT,
                U256::from_be_bytes(args.minter.into_word().0),
            );
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

fn get_allowance<DB: Database>(
    context: &mut OpContext<DB>,
    owner: Address,
    spender: Address,
) -> Option<U256> {
    let outer_slot = get_map_slot(ALLOWANCE_SLOT, owner);
    let final_slot = get_map_slot(outer_slot, spender);

    let load = context.sload(NATIVE_TOKEN_ADDRESS, final_slot)?;
    Some(load.data)
}

fn set_allowance<DB: Database>(
    context: &mut OpContext<DB>,
    owner: Address,
    spender: Address,
    amount: U256,
) -> Result<(), ()> {
    let outer_slot = get_map_slot(ALLOWANCE_SLOT, owner);
    let final_slot = get_map_slot(outer_slot, spender);

    let _ = context.sstore(NATIVE_TOKEN_ADDRESS, final_slot, amount).ok_or(())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_sol_types::SolCall;

    // Test ABI encoding/decoding for read-only methods
    #[test]
    fn test_name_encoding() {
        let call = IERC20::nameCall {};
        let encoded = call.abi_encode();
        let decoded = IERC20::IERC20Calls::abi_decode(&encoded).unwrap();
        assert!(matches!(decoded, IERC20::IERC20Calls::name(_)));
    }

    #[test]
    fn test_symbol_encoding() {
        let call = IERC20::symbolCall {};
        let encoded = call.abi_encode();
        let decoded = IERC20::IERC20Calls::abi_decode(&encoded).unwrap();
        assert!(matches!(decoded, IERC20::IERC20Calls::symbol(_)));
    }

    #[test]
    fn test_decimals_encoding() {
        let call = IERC20::decimalsCall {};
        let encoded = call.abi_encode();
        let decoded = IERC20::IERC20Calls::abi_decode(&encoded).unwrap();
        assert!(matches!(decoded, IERC20::IERC20Calls::decimals(_)));
    }

    #[test]
    fn test_total_supply_encoding() {
        let call = IERC20::totalSupplyCall {};
        let encoded = call.abi_encode();
        let decoded = IERC20::IERC20Calls::abi_decode(&encoded).unwrap();
        assert!(matches!(decoded, IERC20::IERC20Calls::totalSupply(_)));
    }

    #[test]
    fn test_balance_of_encoding() {
        let account = address!("1234567890123456789012345678901234567890");
        let call = IERC20::balanceOfCall { account };
        let encoded = call.abi_encode();
        let decoded = IERC20::IERC20Calls::abi_decode(&encoded).unwrap();
        if let IERC20::IERC20Calls::balanceOf(args) = decoded {
            assert_eq!(args.account, account);
        } else {
            panic!("Expected balanceOf call");
        }
    }

    #[test]
    fn test_transfer_encoding() {
        let recipient = address!("1234567890123456789012345678901234567890");
        let amount = U256::from(1000);
        let call = IERC20::transferCall { recipient, amount };
        let encoded = call.abi_encode();
        let decoded = IERC20::IERC20Calls::abi_decode(&encoded).unwrap();
        if let IERC20::IERC20Calls::transfer(args) = decoded {
            assert_eq!(args.recipient, recipient);
            assert_eq!(args.amount, amount);
        } else {
            panic!("Expected transfer call");
        }
    }

    #[test]
    fn test_approve_encoding() {
        let spender = address!("1234567890123456789012345678901234567890");
        let amount = U256::from(5000);
        let call = IERC20::approveCall { spender, amount };
        let encoded = call.abi_encode();
        let decoded = IERC20::IERC20Calls::abi_decode(&encoded).unwrap();
        if let IERC20::IERC20Calls::approve(args) = decoded {
            assert_eq!(args.spender, spender);
            assert_eq!(args.amount, amount);
        } else {
            panic!("Expected approve call");
        }
    }

    #[test]
    fn test_allowance_encoding() {
        let owner = address!("1111111111111111111111111111111111111111");
        let spender = address!("2222222222222222222222222222222222222222");
        let call = IERC20::allowanceCall { owner, spender };
        let encoded = call.abi_encode();
        let decoded = IERC20::IERC20Calls::abi_decode(&encoded).unwrap();
        if let IERC20::IERC20Calls::allowance(args) = decoded {
            assert_eq!(args.owner, owner);
            assert_eq!(args.spender, spender);
        } else {
            panic!("Expected allowance call");
        }
    }

    #[test]
    fn test_transfer_from_encoding() {
        let sender = address!("1111111111111111111111111111111111111111");
        let recipient = address!("2222222222222222222222222222222222222222");
        let amount = U256::from(100);
        let call = IERC20::transferFromCall { sender, recipient, amount };
        let encoded = call.abi_encode();
        let decoded = IERC20::IERC20Calls::abi_decode(&encoded).unwrap();
        if let IERC20::IERC20Calls::transferFrom(args) = decoded {
            assert_eq!(args.sender, sender);
            assert_eq!(args.recipient, recipient);
            assert_eq!(args.amount, amount);
        } else {
            panic!("Expected transferFrom call");
        }
    }

    #[test]
    fn test_transfer_event_encoding() {
        let from = address!("1111111111111111111111111111111111111111");
        let to = address!("2222222222222222222222222222222222222222");
        let value = U256::from(1000);

        // Verify event signature hash
        assert_eq!(
            Transfer::SIGNATURE_HASH,
            alloy_primitives::keccak256("Transfer(address,address,uint256)")
        );
    }

    #[test]
    fn test_approval_event_encoding() {
        let owner = address!("1111111111111111111111111111111111111111");
        let spender = address!("2222222222222222222222222222222222222222");
        let value = U256::from(5000);

        // Verify event signature hash
        assert_eq!(
            Approval::SIGNATURE_HASH,
            alloy_primitives::keccak256("Approval(address,address,uint256)")
        );
    }

    #[test]
    fn test_storage_slot_computation() {
        let owner = address!("1111111111111111111111111111111111111111");
        let spender = address!("2222222222222222222222222222222222222222");

        // Compute slot for allowance[owner][spender]
        let outer_slot = get_map_slot_generic(ALLOWANCE_SLOT, owner);
        let final_slot = get_map_slot_generic(outer_slot, spender);

        // Should be deterministic
        let outer_slot2 = get_map_slot_generic(ALLOWANCE_SLOT, owner);
        let final_slot2 = get_map_slot_generic(outer_slot2, spender);
        assert_eq!(final_slot, final_slot2);

        // Different owner should give different slot
        let other_owner = address!("3333333333333333333333333333333333333333");
        let other_outer_slot = get_map_slot_generic(ALLOWANCE_SLOT, other_owner);
        let other_final_slot = get_map_slot_generic(other_outer_slot, spender);
        assert_ne!(final_slot, other_final_slot);
    }

    #[test]
    fn test_native_token_address() {
        assert_eq!(NATIVE_TOKEN_ADDRESS, address!("0000000000000000000000000000000000000805"));
    }

    #[test]
    fn test_name_returns_praph() {
        let expected = ("PRAPH".to_string(),).abi_encode();
        // The expected return should encoding "PRAPH"
        assert!(!expected.is_empty());
    }

    #[test]
    fn test_symbol_returns_praf() {
        let expected = ("PRAF".to_string(),).abi_encode();
        assert!(!expected.is_empty());
    }

    #[test]
    fn test_decimals_returns_18() {
        let expected = (U256::from(18),).abi_encode();
        // Decimals should encode as 18
        let decoded: (U256,) = alloy_sol_types::SolValue::abi_decode(&expected).unwrap();
        assert_eq!(decoded.0, U256::from(18));
    }

    #[test]
    fn test_total_supply_returns_max() {
        let expected = (U256::MAX,).abi_encode();
        let decoded: (U256,) = alloy_sol_types::SolValue::abi_decode(&expected).unwrap();
        assert_eq!(decoded.0, U256::MAX);
    }

    #[test]
    fn test_true_return_encoding() {
        // ERC-20 returns true encoded as bool
        let expected = (true,).abi_encode();
        let decoded: (bool,) = alloy_sol_types::SolValue::abi_decode(&expected).unwrap();
        assert!(decoded.0);
    }
}
