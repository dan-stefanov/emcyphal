//! Type-aware object that manages (de)segmentation and a transfer queue.
//!
//! Each endpoint type has a dedicated buffer type

use core::marker::PhantomData;
use core::task::{Context, Poll};

use crate::core::PrioritySet;
use crate::endpoint::TransferMeta;
use crate::format::TransferCrc;
use crate::frame::Frame;
use crate::frame::Mtu;

mod gather;
mod rx_priority_fifo;
mod scatter;

pub mod rx_msg;
pub mod rx_req;
pub mod tx_msg;
pub mod tx_resp;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum BufferError {
    PriorityNotReady,
    MessageAddressSpecified,
    ServiceAddressNotSpecified,
    ScratchpadNotInitialized,
}

/// Covariant guard object
pub(crate) struct BufferToken<'a> {
    _phantom: PhantomData<&'a ()>,
}

impl<'a> BufferToken<'a> {
    // Safety: The object should not outlive the guarded buffers
    unsafe fn create() -> Self {
        Self {
            _phantom: PhantomData,
        }
    }

    pub(crate) fn reborrow(&mut self) -> BufferToken<'_> {
        unsafe { BufferToken::create() }
    }
}

pub(crate) trait DynamicRxBuffer {
    fn push_frame(&mut self, frame: &Frame, mtu: Mtu);

    fn pop_readiness(&self) -> PrioritySet;

    fn poll_pop_readiness(
        &mut self,
        cx: &mut Context,
        priority_mask: PrioritySet,
    ) -> Poll<PrioritySet>;

    fn try_pop<'a>(
        &mut self,
        token: BufferToken<'a>,
        priority_mask: PrioritySet,
    ) -> Option<(TransferMeta, &'a [u8])>;
}

pub(crate) trait DynamicTxBuffer {
    fn pop_readiness(&self) -> PrioritySet;
    fn try_pop(&mut self, priority_mask: PrioritySet, mtu: Mtu) -> Option<Frame>;

    fn is_empty(&self) -> bool;
    fn poll_is_empty(&mut self, cx: &mut Context<'_>) -> Poll<()>;

    fn push_readiness(&self) -> PrioritySet;
    fn poll_push_readiness(
        &mut self,
        cx: &mut Context<'_>,
        priority_mask: PrioritySet,
    ) -> Poll<PrioritySet>;
    fn get_scratchpad<'a>(
        &mut self,
        buf_token: BufferToken<'a>,
    ) -> Option<(PrioritySet, &'a mut [u8])>;
    fn try_push(
        &mut self,
        buf_token: BufferToken<'_>,
        meta: TransferMeta,
        length: usize,
        crc: TransferCrc,
    ) -> Result<(), BufferError>;
}
