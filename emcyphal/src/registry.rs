pub(crate) mod rx_msg;
pub(crate) mod rx_req;
pub(crate) mod tx_msg;
mod tx_queue;
pub(crate) mod tx_resp;

#[derive(Debug)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum RegistrationError {
    NoSubjectSlotLeft,
    DataSpecifierOccupied,
}

pub(crate) trait RxRegKind {
    type Entry: Send;
    type Token: Send + Sync;
}

pub(crate) trait RxReg<K: RxRegKind> {
    /// Register Rx endpoint
    ///
    /// If succeeded, the buffer pointer must stay valid till call of rx_unregister
    /// If failed, the buffer will not be affected
    fn register(
        &self,
        entry: core::ptr::NonNull<K::Entry>,
        loop_back: bool,
    ) -> Result<K::Token, RegistrationError>;

    /// Unregister Rx endpoint
    ///
    /// The associated buffer will be dropped in place
    fn unregister(&self, reg_token: &mut K::Token);
    fn pop_readiness(&self, reg_token: &mut K::Token) -> crate::core::PrioritySet;
    fn poll_pop_readiness(
        &self,
        reg_token: &mut K::Token,
        cx: &mut core::task::Context,
        priority_mask: crate::core::PrioritySet,
    ) -> core::task::Poll<crate::core::PrioritySet>;
    fn try_pop<'a>(
        &self,
        reg_token: &mut K::Token,
        buf_token: crate::buffer::BufferToken<'a>,
        priority_mask: crate::core::PrioritySet,
    ) -> Option<(crate::endpoint::TransferMeta, &'a [u8])>;
}

pub(crate) trait TxRegKind {
    type Entry: Send;
    type Token: Send + Sync;
}

pub(crate) trait TxReg<K: TxRegKind> {
    /// Register Tx endpoint
    ///
    /// If succeeded, the buffer pointer must stay valid till call of unregister
    /// If failed, the buffer will be affected (dropped in place)
    fn register(&self, entry: core::ptr::NonNull<K::Entry>) -> Result<K::Token, RegistrationError>;

    /// Unregister Tx endpoint
    ///
    /// The associated buffer will be dropped in place
    fn unregister(&self, reg_token: &mut K::Token);

    fn is_empty(&self, reg_token: &mut K::Token) -> bool;
    fn poll_is_empty(
        &self,
        reg_token: &mut K::Token,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<()>;

    fn push_readiness(&self, reg_token: &mut K::Token) -> crate::core::PrioritySet;
    fn poll_push_readiness(
        &self,
        reg_token: &mut K::Token,
        cx: &mut core::task::Context<'_>,
        priority_mask: crate::core::PrioritySet,
    ) -> core::task::Poll<crate::core::PrioritySet>;
    fn get_scratchpad<'a>(
        &self,
        reg_token: &mut K::Token,
        buf_token: crate::buffer::BufferToken<'a>,
    ) -> Option<(crate::core::PrioritySet, &'a mut [u8])>;
    fn try_push(
        &self,
        reg_token: &mut K::Token,
        buf_token: crate::buffer::BufferToken<'_>,
        meta: crate::endpoint::TransferMeta,
        length: usize,
        crc: crate::format::TransferCrc,
    ) -> Result<(), crate::buffer::BufferError>;
}
