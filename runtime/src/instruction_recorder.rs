use std::{cell::RefCell, rc::Rc};

use solana_sdk::{
    instruction::{CompiledInstruction, Instruction},
    message::Message,
};

/// Records and compiles cross-program invoked instructions
#[derive(Clone)]
pub struct InstructionRecorder<'a> {
    message: &'a Message,
    inner: Rc<RefCell<Vec<CompiledInstruction>>>,
}

impl<'a> InstructionRecorder<'a> {
    pub fn new(message: &'a Message) -> InstructionRecorder<'a> {
        Self {
            message,
            inner: Rc::default(),
        }
    }

    pub(crate) fn into_inner(self) -> Vec<CompiledInstruction> {
        std::mem::take(&mut self.inner.borrow_mut())
    }

    pub(crate) fn record_instruction(&self, instruction: &Instruction) {
        if let Ok(instruction) = self.message.try_compile_instruction(instruction) {
            self.inner.borrow_mut().push(instruction);
        }
    }
}
