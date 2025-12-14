use core::cell::RefCell;
use core::ptr::NonNull;
use core::task::{Context, Poll};
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::RawMutex;
use emcyphal_core::SubjectId;
use emcyphal_driver::internal::DynamicTx;
use intrusive_collections::rbtree::Entry as RBTreeEntry;
use intrusive_collections::{KeyAdapter, RBTree, RBTreeLink, UnsafeRef, intrusive_adapter};

use crate::buffer::{BufferError, BufferToken, DynamicTxBuffer};
use crate::core::PrioritySet;
use crate::endpoint::TransferMeta;
use crate::format::TransferCrc;
use crate::frame::{Frame, Mtu};
use crate::marker::Message;
use crate::registry::{RegistrationError, TxReg, TxRegKind, tx_queue};

impl TxRegKind for Message {
    type Entry = Entry;
    type Token = Token;
}

pub struct Registry<M: RawMutex>(Mutex<M, RefCell<Inner>>);

impl<M: RawMutex> Default for Registry<M> {
    fn default() -> Self {
        Self(Mutex::new(RefCell::new(Default::default())))
    }
}

impl<M: RawMutex> Registry<M> {
    pub fn with_buffer_mut<T>(
        &self,
        reg_token: &mut Token,
        f: impl FnOnce(&mut dyn DynamicTxBuffer) -> T,
    ) -> T {
        self.0.lock(|cell| {
            let mut inner = cell.borrow_mut();
            inner.with_buffer_mut(reg_token, f)
        })
    }
}

impl<M: RawMutex> DynamicTx for Registry<M> {
    fn poll_pop(&self, cx: &mut Context, priority_mask: PrioritySet, mtu: Mtu) -> Poll<Frame> {
        if priority_mask.is_empty() {
            return Poll::Pending;
        }
        self.0.lock(|cell| {
            let mut inner = cell.borrow_mut();
            inner.hub_poll_pop(cx, priority_mask, mtu)
        })
    }
}

impl<M: RawMutex> TxReg<Message> for Registry<M> {
    fn register(&self, entry: NonNull<Entry>) -> Result<Token, RegistrationError> {
        self.0.lock(|cell| {
            let mut inner = cell.borrow_mut();
            inner.reg_register(entry)
        })
    }

    fn unregister(&self, reg_token: &mut Token) {
        self.0.lock(|cell| {
            let mut inner = cell.borrow_mut();
            inner.reg_unregister(reg_token)
        })
    }

    fn is_empty(&self, reg_token: &mut Token) -> bool {
        self.with_buffer_mut(reg_token, |buffer| buffer.is_empty())
    }

    fn poll_is_empty(&self, reg_token: &mut Token, cx: &mut Context<'_>) -> Poll<()> {
        self.with_buffer_mut(reg_token, |buffer| buffer.poll_is_empty(cx))
    }

    fn push_readiness(&self, reg_token: &mut Token) -> PrioritySet {
        self.with_buffer_mut(reg_token, |buffer| buffer.push_readiness())
    }

    fn poll_push_readiness(
        &self,
        reg_token: &mut Token,
        cx: &mut Context<'_>,
        priority_mask: PrioritySet,
    ) -> Poll<PrioritySet> {
        self.with_buffer_mut(reg_token, |buffer| {
            buffer.poll_push_readiness(cx, priority_mask)
        })
    }

    fn get_scratchpad<'a>(
        &self,
        reg_token: &mut Token,
        buf_token: BufferToken<'a>,
    ) -> Option<(PrioritySet, &'a mut [u8])> {
        self.with_buffer_mut(reg_token, |buffer| buffer.get_scratchpad(buf_token))
    }

    fn try_push(
        &self,
        reg_token: &mut Token,
        buf_token: BufferToken<'_>,
        meta: TransferMeta,
        length: usize,
        crc: TransferCrc,
    ) -> Result<(), BufferError> {
        self.0.lock(|cell| {
            let mut inner = cell.borrow_mut();
            let res = inner.with_buffer_mut(reg_token, |buffer| {
                buffer.try_push(buf_token, meta, length, crc)
            });
            if res.is_ok() {
                inner.reg_update_queue_position(reg_token);
            }
            res
        })
    }
}

pub struct Entry {
    subject: SubjectId,
    tree_link: RBTreeLink,
    queue_entry: tx_queue::Entry,
}

impl Entry {
    // Safety: the pointer should be valid till the end of object lifetime
    // The object will drop the buffer in place
    pub(crate) unsafe fn new(
        subject: SubjectId,
        buffer: NonNull<dyn DynamicTxBuffer + Send>,
    ) -> Self {
        Self {
            subject,
            tree_link: Default::default(),
            queue_entry: unsafe { tx_queue::Entry::new(buffer) },
        }
    }
}

intrusive_adapter!(RxTreeAdapter = UnsafeRef<Entry>: Entry {tree_link: RBTreeLink});

impl<'a> KeyAdapter<'a> for RxTreeAdapter {
    type Key = SubjectId;
    fn get_key(&self, x: &'a Entry) -> Self::Key {
        x.subject
    }
}

pub struct Token(*const Entry);

// Safety: only the Entry owning structure can access the pointer
unsafe impl Send for Token {}

// Safety: only the Entry owning structure can access the pointer
unsafe impl Sync for Token {}

#[derive(Default)]
struct Inner {
    entry_tree: RBTree<RxTreeAdapter>,
    queue: tx_queue::TxQueue,
}

impl Inner {
    fn reg_register(&mut self, entry_ptr: NonNull<Entry>) -> Result<Token, RegistrationError> {
        let entry_ptr = entry_ptr.as_ptr().cast_const();
        let elem = unsafe { UnsafeRef::from_raw(entry_ptr) };
        match self.entry_tree.entry(&elem.subject) {
            RBTreeEntry::Vacant(cursor) => {
                cursor.insert(elem);
            }
            RBTreeEntry::Occupied(_) => {
                return Err(RegistrationError::DataSpecifierOccupied);
            }
        }

        let queue_ptr = unsafe { &raw const (*entry_ptr).queue_entry };
        self.queue.reg_register(queue_ptr);
        Ok(Token(entry_ptr))
    }

    fn reg_unregister(&mut self, token: &mut Token) {
        let mut cursor = unsafe { self.entry_tree.cursor_mut_from_ptr(token.0) };
        unwrap!(cursor.remove());

        let queue_ptr = unsafe { &raw const (*token.0).queue_entry };
        self.queue.reg_unregister(queue_ptr);

        unsafe { token.0.cast_mut().drop_in_place() };
    }

    fn reg_update_queue_position(&mut self, token: &mut Token) {
        let queue_ptr = unsafe { &raw const (*token.0).queue_entry };
        self.queue.reg_update_queue_position(queue_ptr);
    }

    fn with_buffer_mut<T>(
        &mut self,
        token: &mut Token,
        f: impl FnOnce(&mut dyn DynamicTxBuffer) -> T,
    ) -> T {
        let queue_ptr = unsafe { &raw const (*token.0).queue_entry };
        self.queue.with_buffer_mut(queue_ptr, f)
    }

    fn hub_poll_pop(
        &mut self,
        cx: &mut Context<'_>,
        priority_mask: PrioritySet,
        mtu: Mtu,
    ) -> Poll<Frame> {
        self.queue.hub_poll_pop(cx, priority_mask, mtu)
    }
}

// Safety: The object own a unique references to all registered entries.
unsafe impl Send for Inner {}
