//! Higher-level API handles for networking

use embassy_futures::select::{Either, select};
use emcyphal_core::ServiceId;
use emcyphal_encoding::{Deserialize, Serialize};

use crate::buffer::{self, BufferError};
use crate::core::{Priority, PrioritySet, SubjectId, TransferId};
use crate::endpoint::{Rx, Transfer, TransferMeta, Tx};
use crate::marker::{Message, Request, Response};
use crate::node::Hub;
use crate::time::{Duration, Instant};

pub use crate::registry::RegistrationError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum MessageError {
    NotReady,
}

/// Receives messages on a given subject
///
/// A segmented transfer that is not assembled within `timeout` is discarded.
/// Transfers within a single session separated by more than `timeout` are considered unique and
/// will be delivered regardless of `transfer_id`.
///
/// The transfer address is set to the source address, which is `None` for anonymous messages.
///
/// The transfer timestamp is set to the reception instant. For loop-back transfers,
/// it is the instant of successful transmission.
///
/// # Examples:
///
/// ```
/// use emcyphal::buffer;
/// use emcyphal::core::SubjectId;
/// use emcyphal::data_types::ByteArray;
/// use emcyphal::node::Hub;
/// use emcyphal::socket::Subscriber;
/// use emcyphal::time::Duration;
///
/// async fn run_subscriber(hub: Hub<'static>) {
///     let mut rx_buffer = buffer::rx_msg::Watch::<ByteArray, 10>::new();
///     let mut subscriber = Subscriber::create(
///         hub,
///         &mut rx_buffer,
///         SubjectId::new(10).unwrap(),
///         Duration::from_secs(2),
///     )
///     .unwrap();
///     loop {
///         let message = subscriber.pop().await;
///         println!("Received a message: {:?}", &message);
///     }
/// }
/// ```
pub struct Subscriber<'a, T> {
    endpoint: Rx<'a, Message, T>,
}

impl<'a, T: Deserialize> Subscriber<'a, T> {
    /// Subscribes to regular (non-loop-back) transfers.
    ///
    /// Multiple endpoints may subscribe to the same `subject`.
    /// A message will be delivered to each one.
    ///
    /// A node has a limited number of slots for _unique_ subjects.
    /// Fails with `NoSubjectSlotLeft` when slots are exhausted.
    pub fn create(
        hub: Hub<'a>,
        buffer: &'a mut dyn buffer::rx_msg::Buffer<T>,
        subject: SubjectId,
        timeout: Duration,
    ) -> Result<Self, RegistrationError> {
        let endpoint = Rx::create_message(hub, buffer, subject, timeout)?;

        Ok(Self { endpoint })
    }

    /// Returns an enqueued message.
    ///
    /// Skips transfers with deserialization errors
    pub fn try_pop(&mut self) -> Option<T> {
        loop {
            let res = self.endpoint.try_pop(PrioritySet::ALL)?;
            if let Ok(transfer) = res {
                return Some(transfer.payload);
            }
        }
    }

    /// Asynchronously receives a message.
    ///
    /// Skips transfers with deserialization errors
    pub async fn pop(&mut self) -> T {
        loop {
            let res = self.endpoint.pop(PrioritySet::ALL).await;
            if let Ok(transfer) = res {
                return transfer.payload;
            }
        }
    }
}

/// Transmits messages on a given subject
///
/// A transfer or its segment that is not sent before `timeout` expires
/// (e.g., due to higher-priority traffic occupying the bus) will be dropped.
///
/// The object handles `transfer_id` management.
/// Note: [1, sec. 4.1.1.7] requires transfer_id to be consistent during network operation;
/// therefore, a publisher should be initialized once per subject.
///
/// # Examples:
///
/// ```
/// use emcyphal::buffer;
/// use emcyphal::core::{Priority, SubjectId};
/// use emcyphal::data_types::ByteArray;
/// use emcyphal::node::Hub;
/// use emcyphal::socket::Publisher;
/// use emcyphal::time::Duration;
/// use embassy_time::Timer;
///
/// async fn run_publisher(hub: Hub<'static>) {
///     let mut tx_buffer = buffer::tx_msg::Blocking::<ByteArray>::new();
///     let mut publisher = Publisher::create(
///         hub,
///         &mut tx_buffer,
///         SubjectId::new(10).unwrap(),
///         Priority::Nominal,
///         Duration::from_secs(2),
///         true,
///     )
///     .unwrap();
///
///     loop {
///         let message = ByteArray {
///             bytes: heapless::Vec::from_slice(&[0x05]).unwrap(),
///         };
///
///         println!("Send a message: {:?}", &message);
///         publisher.push(message).await;
///
///         Timer::after(Duration::from_secs(2)).await;
///     }
/// }
/// ```
///
/// # References:
///
/// * \[1\] Cyphal Specification v1.0
///   <https://opencyphal.org/specification/Cyphal_Specification.pdf>
pub struct Publisher<'a, T> {
    endpoint: Tx<'a, Message, T>,
    priority: Priority,
    timeout: Duration,
    loop_back: bool,
    transfer_id: TransferId,
}

impl<'a, T: Serialize> Publisher<'a, T> {
    /// Creates a message publishing session.
    ///
    /// Only a single endpoint is allowed per `subject`.
    /// Fails with `DataSpecifierOccupied` if the subject is already occupied.
    pub fn create(
        hub: Hub<'a>,
        buffer: &'a mut dyn buffer::tx_msg::Buffer<T>,
        subject: SubjectId,
        priority: Priority,
        timeout: Duration,
        loop_back: bool,
    ) -> Result<Self, RegistrationError> {
        let endpoint = Tx::create_message(hub, buffer, subject, priority)?;

        Ok(Self {
            endpoint,
            priority,
            timeout,
            loop_back,
            transfer_id: TransferId::default(),
        })
    }

    /// Attempts to place a transfer into the queue.
    pub fn try_push(&mut self, data: T) -> Result<(), MessageError> {
        let deadline = Instant::now().saturating_add(self.timeout);
        let transfer = Transfer {
            meta: TransferMeta {
                priority: self.priority,
                address: None,
                transfer_id: self.transfer_id,
                timestamp: deadline,
                loop_back: self.loop_back,
            },
            payload: data,
        };

        match self.endpoint.try_push(transfer) {
            Ok(()) => {
                self.transfer_id = self.transfer_id.next();
                Ok(())
            }
            Err(BufferError::PriorityNotReady) => Err(MessageError::NotReady),
            _ => unreachable!(),
        }
    }

    /// Asynchronously places a transfer into the queue.
    pub async fn push(&mut self, data: T) {
        self.endpoint
            .wait_push_readiness(PrioritySet::new_eq(self.priority))
            .await;
        unwrap!(self.try_push(data))
    }
}

/// A request-answering worker
///
/// A segmented request transfer that is not assembled within `req_timeout` is discarded.
/// Requests within a single session separated by more than `req_timeout` are considered unique
/// and will be processed regardless of `transfer_id`.
///
/// The response timeout starts at request reception.
/// A response or its segment that is not sent before `resp_timeout` expires
/// (e.g., due to higher-priority traffic occupying the bus) will be dropped.
///
/// The responder uses a single handle instance for all priorities.
/// The handle should be fast to avoid significant priority inversion.
///
/// The responder processes each priority queue independently; a lower-priority request
/// can be executed even when a higher-priority queue is full.
///
/// # Examples
///
/// ```
/// use emcyphal::buffer;
/// use emcyphal::core::ServiceId;
/// use emcyphal::data_types::ByteArray;
/// use emcyphal::node::Hub;
/// use emcyphal::socket::Responder;
/// use emcyphal::time::Duration;
///
/// async fn run_responder(hub: Hub<'static>) {
///     let mut req_buffer = buffer::rx_req::PriorityFifo::<ByteArray, 4, 4>::new();
///     let mut resp_buffer = buffer::tx_resp::Blocking::<ByteArray>::new();
///     let mut responder = Responder::create(
///         hub,
///         &mut req_buffer,
///         &mut resp_buffer,
///         ServiceId::new(10).unwrap(),
///         Duration::from_secs(2),
///         Duration::from_secs(2),
///     )
///     .unwrap();
///
///     responder
///         .run(async |req| {
///             let mut resp = req.clone();
///             for byte in resp.bytes.iter_mut() {
///                 *byte = byte.wrapping_add(1);
///             }
///             resp
///         })
///         .await;
/// }
/// ```
pub struct Responder<'a, Req, Resp> {
    req: Rx<'a, Request, Req>,
    resp: Tx<'a, Response, Resp>,
    resp_timeout: Duration,
}

impl<'a, Req: Deserialize, Resp: Serialize> Responder<'a, Req, Resp> {
    /// Creates a service responding session.
    ///
    /// Only a single responder is allowed per `service`.
    /// Fails with `DataSpecifierOccupied` if the service is already occupied.
    pub fn create(
        hub: Hub<'a>,
        req_buffer: &'a mut dyn buffer::rx_req::Buffer<Req>,
        resp_buffer: &'a mut dyn buffer::tx_resp::Buffer<Resp>,
        service_id: ServiceId,
        req_timeout: Duration,
        resp_timeout: Duration,
    ) -> Result<Self, RegistrationError> {
        let req = Rx::create_request(hub, req_buffer, service_id, req_timeout)?;
        let resp = Tx::create_response(hub, resp_buffer, service_id)?;

        Ok(Self {
            req,
            resp,
            resp_timeout,
        })
    }

    /// Runs the service responder.
    ///
    /// Calls the handle once for each request and waits for completion.
    ///
    /// Uses a single handle instance for all priorities.
    /// The handle should be fast to avoid significant priority inversion.
    pub async fn run(&mut self, mut handle: impl core::ops::AsyncFnMut(&Req) -> Resp) {
        loop {
            self.process_request(&mut handle).await;
        }
    }

    async fn process_request(&mut self, handle: &mut impl core::ops::AsyncFnMut(&Req) -> Resp) {
        let mut req_priorities = self.req.wait_pop_readiness(PrioritySet::ALL).await;
        let mut resp_priorities = self.resp.wait_push_readiness(PrioritySet::ALL).await;

        while (req_priorities & resp_priorities).is_empty() {
            match select(
                self.req.wait_pop_readiness(req_priorities),
                self.resp.wait_push_readiness(resp_priorities),
            )
            .await
            {
                Either::First(priority) => req_priorities |= priority,
                Either::Second(priority) => resp_priorities |= priority,
            }
        }

        // Fetch may fail due to frame replacement in buffer or deserialization error
        let req_transfer = match self.req.try_pop(resp_priorities) {
            Some(Ok(transfer)) => transfer,
            _ => return,
        };
        let resp_payload = handle(&req_transfer.payload).await;
        let resp_transfer = Transfer {
            meta: TransferMeta {
                priority: req_transfer.meta.priority,
                address: req_transfer.meta.address,
                transfer_id: req_transfer.meta.transfer_id,
                timestamp: req_transfer
                    .meta
                    .timestamp
                    .saturating_add(self.resp_timeout),
                loop_back: false,
            },
            payload: resp_payload,
        };
        unwrap!(self.resp.try_push(resp_transfer));
    }
}
