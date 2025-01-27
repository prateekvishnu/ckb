use crate::{verify_env::TxVerifyEnv, ScriptError};
use byteorder::{ByteOrder, LittleEndian};
use ckb_chain_spec::consensus::Consensus;
use ckb_types::core::TransactionView;
use ckb_vm::{
    instructions::{extract_opcode, i, m, rvc, Instruction, Itype},
    machine::VERSION0,
    registers::ZERO,
};
use ckb_vm_definitions::instructions as insts;
use goblin::elf::{section_header::SHF_EXECINSTR, Elf};

#[cfg(test)]
mod tests;

pub(crate) const CKB_VM_ISSUE_92: &str = "https://github.com/nervosnetwork/ckb-vm/issues/92";

/// Ill formed transactions checker.
pub struct IllTransactionChecker<'a> {
    tx: &'a TransactionView,
    consensus: &'a Consensus,
    tx_env: &'a TxVerifyEnv,
}

impl<'a> IllTransactionChecker<'a> {
    /// Creates the checker for a transaction.
    pub fn new(tx: &'a TransactionView, consensus: &'a Consensus, tx_env: &'a TxVerifyEnv) -> Self {
        IllTransactionChecker {
            tx,
            consensus,
            tx_env,
        }
    }

    /// Checks whether the transaction is ill formed.
    pub fn check(&self) -> Result<(), ScriptError> {
        let epoch_number = self.tx_env.epoch_number_without_proposal_window();
        let hardfork_switch = self.consensus.hardfork_switch();
        // Assume that after ckb2021 is activated, developers will only upload code for vm v1.
        if !hardfork_switch.is_vm_version_1_and_syscalls_2_enabled(epoch_number) {
            // IllTransactionChecker is only for vm v0
            for (i, data) in self.tx.outputs_data().into_iter().enumerate() {
                IllScriptChecker::new(&data.raw_data(), i).check()?;
            }
        }
        Ok(())
    }
}

struct IllScriptChecker<'a> {
    data: &'a [u8],
    index: usize,
}

impl<'a> IllScriptChecker<'a> {
    pub fn new(data: &'a [u8], index: usize) -> Self {
        IllScriptChecker { data, index }
    }

    pub fn check(&self) -> Result<(), ScriptError> {
        if self.data.is_empty() {
            return Ok(());
        }
        let elf = match Elf::parse(self.data) {
            Ok(elf) => elf,
            // If the data cannot be parsed as ELF format, we will treat
            // it as a non-script binary data. The checking will be skipped
            // here.
            Err(_) => return Ok(()),
        };
        for section_header in elf.section_headers {
            if section_header.sh_flags & u64::from(SHF_EXECINSTR) != 0 {
                let mut pc = section_header.sh_offset;
                let end = section_header.sh_offset + section_header.sh_size;
                while pc < end {
                    let (option_instruction, len) = self.decode_instruction(pc);
                    if let Some(i) = option_instruction {
                        if extract_opcode(i) == insts::OP_JALR {
                            let i = Itype(i);
                            if i.rs1() == i.rd() && i.rd() != ZERO {
                                return Err(ScriptError::EncounteredKnownBugs(
                                    CKB_VM_ISSUE_92.to_string(),
                                    self.index,
                                ));
                            }
                        };
                    }
                    pc += len;
                }
            }
        }
        Ok(())
    }

    fn decode_instruction(&self, pc: u64) -> (Option<Instruction>, u64) {
        if pc + 2 > self.data.len() as u64 {
            return (None, 2);
        }
        let mut i = u32::from(LittleEndian::read_u16(&self.data[pc as usize..]));
        let len = if i & 0x3 == 0x3 { 4 } else { 2 };
        if len == 4 {
            if pc + 4 > self.data.len() as u64 {
                return (None, 4);
            }
            i = LittleEndian::read_u32(&self.data[pc as usize..]);
        }
        let factories = [rvc::factory::<u64>, i::factory::<u64>, m::factory::<u64>];
        for factory in &factories {
            if let Some(instruction) = factory(i, VERSION0) {
                return (Some(instruction), len);
            }
        }
        (None, len)
    }
}
