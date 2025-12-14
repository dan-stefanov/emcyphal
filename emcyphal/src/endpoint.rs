//! Low-level API handles for sending and receiving transfers

use core::future::poll_fn;
use core::marker::PhantomData;
use core::task::{Context, Poll};
use emcyphal_encoding::{Deserialize, Serialize};

use crate::buffer::{self, BufferError};
use crate::core::{NodeId, Priority, PrioritySet, ServiceId, SubjectId, TransferId};
use crate::format::TransferCrc;
use crate::marker::{Message, Request, Response};
use crate::node::Hub;
use crate::registry::{self, RxRegKind, TxRegKind};
use crate::time::{Duration, Instant};

pub use crate::registry::RegistrationError;
pub use emcyphal_encoding::DeserializeError;

/// Transfer Metadata
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct TransferMeta {
    pub priority: Priority,
    pub address: Option<NodeId>,
    pub transfer_id: TransferId,
    pub timestamp: Instant,
    pub loop_back: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transfer<T> {
    pub meta: TransferMeta,
    pub payload: T,
}

#[allow(private_bounds)]
pub trait RxEndpointKind: RxRegKind {}

impl RxEndpointKind for Message {}
impl RxEndpointKind for Request {}

#[allow(private_bounds)]
pub trait TxEndpointKind: TxRegKind {}

impl TxEndpointKind for Message {}
impl TxEndpointKind for Response {}

/// Receives transfers on a given data specifier (subject or service)
///
/// A segmented transfer that is not assembled within `timeout` is discarded.
/// Transfers within a single session separated by more than `timeout` are considered unique and
/// will be delivered regardless of `transfer_id`.
///
/// The transfer address is set to the source address. It is always Some for services but can
/// be None for anonymous messages.
///
/// The transfer timestamp is set to the reception instant. For loop-back transfers,
/// it is the instant of successful transmission.
pub struct Rx<'a, K: RxEndpointKind, T> {
    reg: &'a (dyn registry::RxReg<K> + Sync),
    reg_token: K::Token,
    buf_token: buffer::BufferToken<'a>,
    _phantom: PhantomData<T>,
}

impl<'a, T: Deserialize> Rx<'a, Message, T> {
    /// Creates a message-receiving endpoint.
    ///
    /// Multiple endpoints are allowed to share the same `subject`.
    /// A message will be delivered to each one.
    ///
    /// A node has a limited number of slots for _unique_ non-loop-back subjects.
    /// Fails with `NoSubjectSlotLeft` when slots are exhausted.
    pub fn create_message(
        hub: Hub<'a>,
        buffer: &'a mut dyn buffer::rx_msg::Buffer<T>,
        subject: SubjectId,
        timeout: Duration,
    ) -> Result<Self, RegistrationError> {
        let reg = hub.rx_msg();
        // Safety: Rx does not outlive the entry reference and unregisters it in Drop.
        let (entry_ptr, buf_token) = unsafe { buffer.init(subject, timeout) };
        let res = reg.register(entry_ptr, false);
        if res.is_err() {
            unsafe { entry_ptr.drop_in_place() };
        }

        Ok(Self {
            reg,
            reg_token: res?,
            buf_token,
            _phantom: PhantomData,
        })
    }

    /// Creates a loop-back message-receiving endpoint.
    ///
    /// Multiple endpoints are allowed to share the same `subject`.
    /// A message will be delivered to each one.
    ///
    /// A node does not limit the number of loop-back subscriptions.
    /// Loop-back subjects do not occupy slots for regular subjects.
    pub fn create_message_loop_back(
        hub: Hub<'a>,
        buffer: &'a mut dyn buffer::rx_msg::Buffer<T>,
        subject: SubjectId,
        timeout: Duration,
    ) -> Result<Self, RegistrationError> {
        let reg = hub.rx_msg();
        // Safety: Rx does not outlive the entry reference and unregisters it in Drop.
        let (entry_ptr, buf_token) = unsafe { buffer.init(subject, timeout) };
        let res = reg.register(entry_ptr, true);
        if res.is_err() {
            unsafe { entry_ptr.drop_in_place() };
        }

        Ok(Self {
            reg,
            reg_token: res?,
            buf_token,
            _phantom: PhantomData,
        })
    }
}

impl<'a, T: Deserialize> Rx<'a, Request, T> {
    /// Creates a request-receiving endpoint.
    ///
    /// Only a single endpoint is allowed per service.
    /// Fails with `DataSpecifierOccupied` if the service is already occupied.
    /// Loop-back transfers are not supported.
    pub fn create_request(
        hub: Hub<'a>,
        buffer: &'a mut dyn buffer::rx_req::Buffer<T>,
        service: ServiceId,
        timeout: Duration,
    ) -> Result<Self, RegistrationError> {
        let reg = hub.rx_req();

        // Safety: Rx does not outlive the entry reference and unregisters the buffer in Drop.
        let (entry_ptr, buf_token) = unsafe { buffer.init(service, timeout) };
        let res = reg.register(entry_ptr, false);
        if res.is_err() {
            unsafe { entry_ptr.drop_in_place() };
        }

        Ok(Self {
            reg,
            reg_token: res?,
            buf_token,
            _phantom: PhantomData,
        })
    }
}

impl<'a, K: RxEndpointKind, T: Deserialize> Rx<'a, K, T> {
    /// Returns a set of priorities that have queued transfers.
    ///
    /// The next `try_pop` may fail if, for example, the last transfer for the reported priority
    /// was replaced by a higher-priority one.
    pub fn pop_readiness(&mut self) -> PrioritySet {
        self.reg.pop_readiness(&mut self.reg_token)
    }

    fn poll_pop_readiness(
        &mut self,
        cx: &mut Context<'_>,
        priority_mask: PrioritySet,
    ) -> Poll<PrioritySet> {
        self.reg
            .poll_pop_readiness(&mut self.reg_token, cx, priority_mask)
    }

    /// Asynchronously waits for a transfer
    ///
    /// The method checks for a transfer with either priority from the mask.
    pub async fn wait_pop_readiness(&mut self, priority_mask: PrioritySet) -> PrioritySet {
        poll_fn(|cx| self.poll_pop_readiness(cx, priority_mask)).await
    }

    /// Fetches and deserialize a transfer from the queue.
    ///
    /// The method checks for a transfer with either priority from the mask. Returns the one with
    /// the highest priority.
    pub fn try_pop(
        &mut self,
        priority_mask: PrioritySet,
    ) -> Option<Result<Transfer<T>, DeserializeError>> {
        let (meta, buffer) = self.reg.try_pop(
            &mut self.reg_token,
            self.buf_token.reborrow(),
            priority_mask,
        )?;

        Some(T::deserialize_from_bytes(buffer).map(|payload| Transfer { meta, payload }))
    }

    /// Asynchronously receives and deserialize a transfer.
    ///
    /// The method checks for a transfer with either priority from the mask. Returns the one with
    /// the highest priority.
    pub async fn pop(
        &mut self,
        priority_mask: PrioritySet,
    ) -> Result<Transfer<T>, DeserializeError> {
        loop {
            self.wait_pop_readiness(priority_mask).await;
            if let Some(res) = self.try_pop(priority_mask) {
                return res;
            }
        }
    }
}

impl<'a, K: RxEndpointKind, T> Drop for Rx<'a, K, T> {
    fn drop(&mut self) {
        self.reg.unregister(&mut self.reg_token);
    }
}

/// Transmits transfers on a given data specifier (subject or service)
///
/// A transfer `timestamp` is interpreted as a deadline. If a transfer or its segment is not sent
/// before the deadline (e.g., due to higher-priority traffic occupying the bus), it will be dropped.
///
/// A transfer `address` is interpreted as the destination. It should be None for messages and Some
/// for service transfers.
///
/// Message priority is fixed on endpoint creation. The buffer will not accept any other priority.
/// Response priority should match the one from the received request.
///
/// The `transfer_id` for messages should grow sequentially.
/// Note: [1, sec. 4.1.1.7] requires `transfer_id` to be consistent during network operation.
/// If the node has published on the same `subject` before, a newly created endpoint should
/// continue the transfer_id sequence.
///
/// The `transfer_id` for responses should match the one from the received request.
///
/// The node fetches segments from the highest priority only. If the driver queue for this priority
/// is full, lower-priority responses may remain in the queue indefinitely.
pub struct Tx<'a, K: TxEndpointKind, T> {
    reg: &'a (dyn registry::TxReg<K> + Sync),
    reg_token: K::Token,
    buf_token: buffer::BufferToken<'a>,
    _phantom: PhantomData<T>,
}

impl<'a, T: Serialize> Tx<'a, Message, T> {
    /// Creates a message publishing session.
    ///
    /// Only a single endpoint is allowed per subject.
    /// Fails with `DataSpecifierOccupied` if the subject is already occupied.
    pub fn create_message(
        hub: Hub<'a>,
        buffer: &'a mut dyn buffer::tx_msg::Buffer<T>,
        subject: SubjectId,
        priority: Priority,
    ) -> Result<Self, RegistrationError> {
        let reg = hub.tx_msg();
        // Safety: Tx does not outlive the buffer reference and unregisters the buffer in Drop.
        let (entry_ptr, buf_token) = unsafe { buffer.init(subject, priority) };
        let res = reg.register(entry_ptr);
        if res.is_err() {
            unsafe { entry_ptr.drop_in_place() };
        }

        Ok(Self {
            reg,
            reg_token: res?,
            buf_token,
            _phantom: PhantomData,
        })
    }
}

impl<'a, T: Serialize> Tx<'a, Response, T> {
    /// Creates a response-transmitting session.
    ///
    /// Multiple endpoints are allowed per service. Collisions are not possible because
    /// a response inherits a temporarily unique (address, transfer_id) pair from the received
    /// request transfer.
    pub fn create_response(
        hub: Hub<'a>,
        buffer: &'a mut dyn buffer::tx_resp::Buffer<T>,
        service: ServiceId,
    ) -> Result<Self, RegistrationError> {
        let reg = hub.tx_resp();

        // Safety: Tx does not outlive the buffer reference and unregisters the buffer in Drop.
        let (entry_ptr, buf_token) = unsafe { buffer.init(service) };
        let res = reg.register(entry_ptr);
        if res.is_err() {
            unsafe { entry_ptr.drop_in_place() };
        }

        Ok(Self {
            reg,
            reg_token: res?,
            buf_token,
            _phantom: PhantomData,
        })
    }
}

impl<'a, K: TxEndpointKind, T: Serialize> Tx<'a, K, T> {
    /// Checks if all pending transfers have been fetched from the buffer.
    ///
    /// The endpoint can be safely dropped now.
    pub fn all_sent(&mut self) -> bool {
        self.reg.is_empty(&mut self.reg_token)
    }

    fn poll_all_sent(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        self.reg.poll_is_empty(&mut self.reg_token, cx)
    }

    /// Asynchronously waits for all pending transfers to be fetched from the buffer.
    pub async fn wait_all_sent(&mut self) {
        poll_fn(|cx| self.poll_all_sent(cx)).await
    }

    /// Returns a set of priorities that the queue is ready to accept.
    ///
    /// The next `try_push` for either reported priority is guaranteed to succeed.
    pub fn push_readiness(&mut self) -> PrioritySet {
        self.reg.push_readiness(&mut self.reg_token)
    }

    fn poll_push_readiness(
        &mut self,
        cx: &mut Context<'_>,
        priority_mask: PrioritySet,
    ) -> Poll<PrioritySet> {
        self.reg
            .poll_push_readiness(&mut self.reg_token, cx, priority_mask)
    }

    /// Asynchronously waits for queue readiness
    ///
    /// The method checks for either priority from the mask.
    pub async fn wait_push_readiness(&mut self, priority_mask: PrioritySet) -> PrioritySet {
        poll_fn(|cx| self.poll_push_readiness(cx, priority_mask)).await
    }

    /// Attempts to place a transfer into the queue.
    ///
    /// Fails if the queue for the transfer priority is not ready.
    pub fn try_push(&mut self, transfer: Transfer<T>) -> Result<(), BufferError> {
        let (priorities, buffer) = self
            .reg
            .get_scratchpad(&mut self.reg_token, self.buf_token.reborrow())
            .ok_or(BufferError::PriorityNotReady)?;
        if !priorities.contains(transfer.meta.priority) {
            return Err(BufferError::PriorityNotReady);
        }

        transfer.payload.serialize_to_bytes(buffer);
        let length = transfer.payload.size_bits().div_ceil(8);
        let mut crc: TransferCrc = Default::default();
        crc.add_bytes(&buffer[..length]);

        self.reg.try_push(
            &mut self.reg_token,
            self.buf_token.reborrow(),
            transfer.meta,
            length,
            crc,
        )
    }
}

impl<'a, K: TxEndpointKind, T> Drop for Tx<'a, K, T> {
    fn drop(&mut self) {
        self.reg.unregister(&mut self.reg_token);
    }
}
