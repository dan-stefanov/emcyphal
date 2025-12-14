use core::cell::RefCell;
use core::ptr::NonNull;
use core::task::{Context, Poll};
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::RawMutex;
use emcyphal_driver::internal::DynamicRx;
use intrusive_collections::rbtree::Entry as RBTreeEntry;
use intrusive_collections::{KeyAdapter, RBTree, RBTreeLink, UnsafeRef, intrusive_adapter};

use crate::buffer::{BufferToken, DynamicRxBuffer};
use crate::core::{PrioritySet, ServiceId};
use crate::endpoint::TransferMeta;
use crate::frame::{DataSpecifier, Frame, Mtu};
use crate::marker::Request;
use crate::registry::{RegistrationError, RxReg, RxRegKind};

impl RxRegKind for Request {
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
        f: impl FnOnce(&mut dyn DynamicRxBuffer) -> T,
    ) -> T {
        self.0.lock(|cell| {
            let mut inner = cell.borrow_mut();
            inner.with_buffer_mut(reg_token, f)
        })
    }
}

impl<M: RawMutex> DynamicRx for Registry<M> {
    fn poll_push(&self, _cx: &mut Context<'_>, frame: &Frame, mtu: Mtu) -> Poll<()> {
        if frame.loop_back {
            return Poll::Ready(());
        }

        self.0.lock(|cell| {
            let mut inner = cell.borrow_mut();
            inner.hub_push(frame, mtu)
        });
        Poll::Ready(())
    }
}

impl<M: RawMutex> RxReg<Request> for Registry<M> {
    fn register(&self, entry: NonNull<Entry>, loop_back: bool) -> Result<Token, RegistrationError> {
        assert!(!loop_back);
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

    fn pop_readiness(&self, reg_token: &mut Token) -> PrioritySet {
        self.with_buffer_mut(reg_token, |buffer| buffer.pop_readiness())
    }

    fn poll_pop_readiness(
        &self,
        reg_token: &mut Token,
        cx: &mut Context,
        priority_mask: PrioritySet,
    ) -> Poll<PrioritySet> {
        self.with_buffer_mut(reg_token, |buffer| {
            buffer.poll_pop_readiness(cx, priority_mask)
        })
    }

    fn try_pop<'a>(
        &self,
        reg_token: &mut Token,
        buf_token: BufferToken<'a>,
        priority_mask: PrioritySet,
    ) -> Option<(TransferMeta, &'a [u8])> {
        self.with_buffer_mut(reg_token, |buffer| buffer.try_pop(buf_token, priority_mask))
    }
}

pub struct Entry {
    service: ServiceId,
    tree_link: RBTreeLink,
    buffer: NonNull<dyn DynamicRxBuffer>,
}

impl Entry {
    pub(crate) unsafe fn new(service: ServiceId, buffer: NonNull<dyn DynamicRxBuffer>) -> Self {
        Self {
            service,
            tree_link: Default::default(),
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

intrusive_adapter!(RxTreeAdapter = UnsafeRef<Entry>: Entry {tree_link: RBTreeLink});

impl<'a> KeyAdapter<'a> for RxTreeAdapter {
    type Key = ServiceId;
    fn get_key(&self, x: &'a Entry) -> Self::Key {
        x.service
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
}

impl Inner {
    fn reg_register(&mut self, entry_ptr: NonNull<Entry>) -> Result<Token, RegistrationError> {
        let entry = unsafe { entry_ptr.as_ref() };
        match self.entry_tree.entry(&entry.service) {
            RBTreeEntry::Vacant(cursor) => {
                let elem = unsafe { UnsafeRef::from_raw(entry_ptr.as_ptr()) };
                cursor.insert(elem);
            }
            RBTreeEntry::Occupied(_) => {
                return Err(RegistrationError::DataSpecifierOccupied);
            }
        }

        Ok(Token(entry_ptr.as_ptr()))
    }

    fn reg_unregister(&mut self, token: &mut Token) {
        let mut cursor = unsafe { self.entry_tree.cursor_mut_from_ptr(token.0) };
        let elem = unwrap!(cursor.remove());
        unsafe { elem.buffer.drop_in_place() };
    }

    fn with_buffer_mut<T>(
        &mut self,
        token: &mut Token,
        f: impl FnOnce(&mut dyn DynamicRxBuffer) -> T,
    ) -> T {
        let entry = unwrap!(unsafe { token.0.as_ref() });
        let mut buffer_ptr = entry.buffer;
        let buffer = unsafe { buffer_ptr.as_mut() };
        f(buffer)
    }

    fn hub_push(&mut self, frame: &Frame, mtu: Mtu) {
        if let DataSpecifier::Request(service) = frame.header.data_spec
            && let Some(entry) = self.entry_tree.find(&service).get()
        {
            let mut buffer_ptr = entry.buffer;
            let buffer = unsafe { buffer_ptr.as_mut() };
            buffer.push_frame(frame, mtu);
        }
    }
}

// Safety: The object own a unique references to all registered entries.
unsafe impl Send for Inner {}
