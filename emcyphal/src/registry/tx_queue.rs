use core::cell::Cell;
use core::ptr::NonNull;
use core::task::{Context, Poll};
use intrusive_collections::{LinkedList, LinkedListLink, UnsafeRef, intrusive_adapter};

use crate::buffer::DynamicTxBuffer;
use crate::core::{Priority, PrioritySet};
use crate::format::is_eot_frame;
use crate::frame::{Frame, Mtu};
use crate::utils::{PriorityArray, PriorityTrigger};

#[derive(Clone, Copy, PartialEq, Eq)]
enum QueueType {
    Started,
    Finished,
}

type QueueId = (Priority, QueueType);

pub struct Entry {
    list_link: LinkedListLink,
    queue_mark: Cell<Option<QueueId>>,
    started: Cell<PrioritySet>,
    buffer: NonNull<dyn DynamicTxBuffer + Send>,
}

impl Entry {
    // Safety: the pointer should be valid till the end of object lifetime
    // The object will drop the buffer in place
    pub unsafe fn new(buffer: NonNull<dyn DynamicTxBuffer + Send>) -> Self {
        Self {
            list_link: Default::default(),
            queue_mark: Default::default(),
            started: Default::default(),
            buffer,
        }
    }
}

impl Drop for Entry {
    fn drop(&mut self) {
        unsafe { self.buffer.drop_in_place() };
    }
}

// Safety: Entry owens an unique reference to the buffer
unsafe impl Send for Entry {}

intrusive_adapter!(RxListAdapter = UnsafeRef<Entry>: Entry {list_link: LinkedListLink});

#[derive(Default)]
pub struct TxQueue {
    queues: Queues,
    non_empty_trigger: PriorityTrigger,
}

impl TxQueue {
    pub fn reg_register(&mut self, entry_ptr: *const Entry) {
        self.reg_update_queue_position(entry_ptr);
    }

    pub fn reg_unregister(&mut self, entry_ptr: *const Entry) {
        self.queues.dequeue(entry_ptr);
    }

    pub fn reg_update_queue_position(&mut self, entry_ptr: *const Entry) {
        let entry = unsafe { entry_ptr.as_ref().unwrap_unchecked() };
        let started = entry.started.get();
        let mut buffer_ptr = entry.buffer;
        let buffer = unsafe { buffer_ptr.as_mut() };
        let new_readiness = buffer.pop_readiness();

        if let Some(priority) = new_readiness.first() {
            let id = if started.contains(priority) {
                (priority, QueueType::Started)
            } else {
                (priority, QueueType::Finished)
            };

            let prev_queue_id = self.queues.element_queue_id(entry_ptr);
            if prev_queue_id != Some(id) {
                self.queues.move_back(id, entry_ptr);
                self.non_empty_trigger.wake(PrioritySet::new_eq(id.0));
            }
        } else {
            self.queues.dequeue(entry_ptr);
        }
    }

    pub fn with_buffer_mut<T>(
        &mut self,
        entry_ptr: *const Entry,
        f: impl FnOnce(&mut dyn DynamicTxBuffer) -> T,
    ) -> T {
        let entry = unsafe { entry_ptr.as_ref().unwrap_unchecked() };
        let mut buffer_ptr = entry.buffer;
        let buffer = unsafe { buffer_ptr.as_mut() };
        f(buffer)
    }

    pub fn hub_poll_pop(
        &mut self,
        cx: &mut Context<'_>,
        priority_mask: PrioritySet,
        mtu: Mtu,
    ) -> Poll<Frame> {
        if let Some(frame) = self.hub_try_pop(priority_mask, mtu) {
            Poll::Ready(frame)
        } else {
            self.non_empty_trigger.register(cx.waker(), priority_mask);
            Poll::Pending
        }
    }

    fn hub_try_pop(&mut self, priority_mask: PrioritySet, mtu: Mtu) -> Option<Frame> {
        let (priority, queue) = Self::first_queue_id(
            self.queues.started_priorities() & priority_mask,
            self.queues.finished_priorities() & priority_mask,
        )?;
        let entry = unwrap!(self.queues.get_front((priority, queue)));

        let entry_ptr: *const Entry = entry;
        let mut buffer_ptr = entry.buffer;
        let buffer = unsafe { buffer_ptr.as_mut() };
        let res = buffer.try_pop(PrioritySet::new_eq(priority), mtu);
        if let Some(frame) = res.as_ref() {
            assert_eq!(frame.header.priority, priority);
            assert!(frame.data.len() <= mtu.into());

            let eot = unwrap!(is_eot_frame(frame));
            entry.started.update(|mut set| {
                if eot {
                    set.remove(priority)
                } else {
                    set.insert(priority)
                }
                set
            });

            if eot {
                // If element stays in `finished` queue, we need to move it back to allow pull
                // evenly from other endpoints
                self.queues.dequeue(entry_ptr);
            }
        }

        self.reg_update_queue_position(entry_ptr);
        res
    }

    fn first_queue_id(started: PrioritySet, finished: PrioritySet) -> Option<QueueId> {
        if let Some(priority) = (started | finished).first() {
            let queue_type = if started.contains(priority) {
                QueueType::Started
            } else {
                QueueType::Finished
            };

            Some((priority, queue_type))
        } else {
            None
        }
    }
}

#[derive(Default)]
struct Queues {
    started: PriorityArray<LinkedList<RxListAdapter>>,
    started_priorities: PrioritySet,
    finished: PriorityArray<LinkedList<RxListAdapter>>,
    finished_priorities: PrioritySet,
}

impl Queues {
    fn started_priorities(&self) -> PrioritySet {
        self.started_priorities
    }

    fn finished_priorities(&self) -> PrioritySet {
        self.finished_priorities
    }

    fn get_front(&mut self, id: QueueId) -> Option<&Entry> {
        let queue = self.get_queue(id);
        queue.front().get()
    }

    fn element_queue_id(&self, entry_ptr: *const Entry) -> Option<QueueId> {
        let entry = unsafe { entry_ptr.as_ref().unwrap_unchecked() };
        entry.queue_mark.get()
    }

    fn move_back(&mut self, id: QueueId, entry_ptr: *const Entry) {
        self.dequeue(entry_ptr);

        let elem = unsafe { UnsafeRef::from_raw(entry_ptr.as_ref().unwrap_unchecked()) };
        elem.queue_mark.set(Some(id));
        self.get_queue(id).push_back(elem);
        self.clear_empty_flag(id);
    }

    fn dequeue(&mut self, entry_ptr: *const Entry) {
        let entry = unsafe { entry_ptr.as_ref().unwrap_unchecked() };
        let prev_queue_id = entry.queue_mark.replace(None);
        if let Some(id) = prev_queue_id {
            let queue = self.get_queue(id);
            let mut cursor = unsafe { queue.cursor_mut_from_ptr(entry) };
            unwrap!(cursor.remove());
            if queue.is_empty() {
                self.set_empty_flag(id);
            }
        }
    }

    fn get_queue(&mut self, id: QueueId) -> &mut LinkedList<RxListAdapter> {
        let (priority, queue_type) = id;
        match queue_type {
            QueueType::Started => &mut self.started[priority],
            QueueType::Finished => &mut self.finished[priority],
        }
    }

    fn set_empty_flag(&mut self, id: QueueId) {
        let (priority, queue_type) = id;
        match queue_type {
            QueueType::Started => self.started_priorities.remove(priority),
            QueueType::Finished => self.finished_priorities.remove(priority),
        }
    }

    fn clear_empty_flag(&mut self, id: QueueId) {
        let (priority, queue_type) = id;
        match queue_type {
            QueueType::Started => self.started_priorities.insert(priority),
            QueueType::Finished => self.finished_priorities.insert(priority),
        }
    }
}
