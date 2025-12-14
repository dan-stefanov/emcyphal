use core::cell::{Cell, RefCell};
use core::ptr::NonNull;
use core::task::{Context, Poll};
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::waitqueue::WakerRegistration;
use emcyphal_driver::internal::{DynamicRx, DynamicRxFilter};
use emcyphal_driver::link::FilterUpdate;
use intrusive_collections::{
    Bound, KeyAdapter, LinkedList, LinkedListLink, RBTree, RBTreeLink, UnsafeRef, intrusive_adapter,
};

use crate::buffer::{BufferToken, DynamicRxBuffer};
use crate::core::{PrioritySet, SubjectId};
use crate::endpoint::TransferMeta;
use crate::frame::{DataSpecifier, Frame, Mtu};
use crate::marker::Message;
use crate::registry::{RegistrationError, RxReg, RxRegKind};

impl RxRegKind for Message {
    type Entry = Entry;
    type Token = Token;
}

pub struct Token {
    inner: InnerToken,
    loop_back: bool,
}

pub struct Registry<M: RawMutex> {
    regular: Mutex<M, RefCell<RegularInner>>,
    loop_back: Mutex<M, RefCell<LoopBackInner>>,
}

impl<M: RawMutex> Registry<M> {
    fn hub_push_regular(&self, subject: SubjectId, frame: &Frame, mtu: Mtu) {
        let started = self.regular.lock(|cell| {
            let mut inner = cell.borrow_mut();
            inner.hub_push_start(subject)
        });
        if started {
            while self.regular.lock(|cell| {
                let mut inner = cell.borrow_mut();
                inner.hub_push_next(frame, mtu)
            }) {}
        }
    }

    fn hub_push_loop_back(&self, subject: SubjectId, frame: &Frame, mtu: Mtu) {
        let started = self.loop_back.lock(|cell| {
            let mut inner = cell.borrow_mut();
            inner.hub_push_start(subject)
        });
        if started {
            while self.loop_back.lock(|cell| {
                let mut inner = cell.borrow_mut();
                inner.hub_push_next(frame, mtu)
            }) {}
        }
    }
}

impl<M: RawMutex> Registry<M> {
    pub fn new(subject_slot_count: usize) -> Self {
        Self {
            regular: Mutex::new(RefCell::new(RegularInner::new(subject_slot_count))),
            loop_back: Mutex::new(RefCell::new(Default::default())),
        }
    }

    // We cannot access the Ency pointed by inner token directly, because it is not Sync
    // A concurrent push can access the same nested buffer
    pub fn with_buffer_mut<T>(
        &self,
        reg_token: &mut Token,
        f: impl FnOnce(&mut dyn DynamicRxBuffer) -> T,
    ) -> T {
        if !reg_token.loop_back {
            self.regular.lock(|cell| {
                let mut inner = cell.borrow_mut();
                inner.with_buffer_mut(&mut reg_token.inner, f)
            })
        } else {
            self.loop_back.lock(|cell| {
                let mut inner = cell.borrow_mut();
                inner.with_buffer_mut(&mut reg_token.inner, f)
            })
        }
    }

    fn hub_push(&self, frame: &Frame, mtu: Mtu) {
        let subject = match frame.header.data_spec {
            DataSpecifier::Message(id) => id,
            _ => {
                return;
            }
        };

        if !frame.loop_back {
            self.hub_push_regular(subject, frame, mtu);
        } else {
            self.hub_push_loop_back(subject, frame, mtu);
        }
    }
}

impl<M: RawMutex> DynamicRxFilter for Registry<M> {
    fn poll_pop(&self, cx: &mut Context<'_>) -> Poll<FilterUpdate> {
        loop {
            let res = self.regular.lock(|cell| {
                let mut inner = cell.borrow_mut();
                inner.hub_poll_pop_filter_update(cx)
            });
            match res {
                Poll::Ready(Ok(filter_update)) => return Poll::Ready(filter_update),
                Poll::Ready(Err(CallAgain)) => continue,
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

impl<M: RawMutex> DynamicRx for Registry<M> {
    fn poll_push(&self, _cx: &mut Context<'_>, frame: &Frame, mtu: Mtu) -> Poll<()> {
        self.hub_push(frame, mtu);
        Poll::Ready(())
    }
}

impl<M: RawMutex> RxReg<Message> for Registry<M> {
    fn register(&self, entry: NonNull<Entry>, loop_back: bool) -> Result<Token, RegistrationError> {
        let inner_token = if loop_back {
            self.loop_back.lock(|cell| {
                let mut inner = cell.borrow_mut();
                inner.reg_register(entry)
            })?
        } else {
            self.regular.lock(|cell| {
                let mut inner = cell.borrow_mut();
                inner.reg_register(entry)
            })?
        };
        Ok(Token {
            inner: inner_token,
            loop_back,
        })
    }

    fn unregister(&self, reg_token: &mut Token) {
        if !reg_token.loop_back {
            self.regular.lock(|cell| {
                let mut inner = cell.borrow_mut();
                inner.reg_unregister(&mut reg_token.inner)
            })
        } else {
            self.loop_back.lock(|cell| {
                let mut inner = cell.borrow_mut();
                inner.reg_unregister(&mut reg_token.inner)
            })
        }
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
    subject: SubjectId,
    tree_link: RBTreeLink,
    list_link: LinkedListLink,
    is_active: Cell<bool>,
    is_head: Cell<bool>,
    buffer: NonNull<dyn DynamicRxBuffer + Send>,
}

impl Entry {
    pub unsafe fn new(subject: SubjectId, buffer: NonNull<dyn DynamicRxBuffer + Send>) -> Self {
        Self {
            subject,
            tree_link: Default::default(),
            list_link: Default::default(),
            is_active: Default::default(),
            is_head: Default::default(),
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
intrusive_adapter!(RxListAdapter = UnsafeRef<Entry>: Entry {list_link: LinkedListLink});

impl<'a> KeyAdapter<'a> for RxTreeAdapter {
    type Key = SubjectId;
    fn get_key(&self, x: &'a Entry) -> Self::Key {
        x.subject
    }
}

struct CallAgain;

struct InnerToken(pub *const Entry);

// Safety: only the Entry owning structure can access the pointer
unsafe impl Send for InnerToken {}

// Safety: only the Entry owning structure can access the pointer
unsafe impl Sync for InnerToken {}

struct RegularInner {
    inactive_entry_tree: RBTree<RxTreeAdapter>,
    active_entry_tree: RBTree<RxTreeAdapter>,
    next_push_ptr: *const Entry,
    clean_up_queue: LinkedList<RxListAdapter>,
    clean_up_last: bool,
    activation_queue: LinkedList<RxListAdapter>,
    free_slot_count: usize,
    filter_update_waker: WakerRegistration,
}

impl RegularInner {
    fn new(subject_slot_count: usize) -> Self {
        Self {
            inactive_entry_tree: Default::default(),
            active_entry_tree: Default::default(),
            next_push_ptr: core::ptr::null(),
            clean_up_queue: Default::default(),
            clean_up_last: false,
            activation_queue: Default::default(),
            free_slot_count: subject_slot_count,
            filter_update_waker: Default::default(),
        }
    }

    fn reg_register(&mut self, entry_ptr: NonNull<Entry>) -> Result<InnerToken, RegistrationError> {
        let elem = unsafe { UnsafeRef::from_raw(entry_ptr.as_ptr()) };
        let token = InnerToken(entry_ptr.as_ptr());

        let mut inactive_cursor = self
            .inactive_entry_tree
            .upper_bound_mut(Bound::Included(&elem.subject));
        let enqueued_subject = inactive_cursor
            .get()
            .is_some_and(|prev_entry| prev_entry.subject == elem.subject);

        if enqueued_subject {
            elem.is_active.set(false);
            elem.is_head.set(false);
            inactive_cursor.insert_after(elem.clone());
            self.activation_queue.push_back(elem);
            return Ok(token);
        }

        let mut active_cursor = self
            .active_entry_tree
            .upper_bound_mut(Bound::Included(&elem.subject));
        let active_subject = active_cursor
            .get()
            .is_some_and(|prev_entry| prev_entry.subject == elem.subject);

        if active_subject {
            elem.is_active.set(true);
            elem.is_head.set(false);
            active_cursor.insert_after(elem);
            return Ok(token);
        }

        if self.free_slot_count == 0 {
            return Err(RegistrationError::NoSubjectSlotLeft);
        }
        self.free_slot_count -= 1;

        elem.is_active.set(false);
        elem.is_head.set(false);
        inactive_cursor.insert_after(elem.clone());
        self.activation_queue.push_back(elem);
        self.filter_update_waker.wake();

        Ok(token)
    }

    fn reg_unregister(&mut self, token: &mut InnerToken) {
        let entry = unwrap!(unsafe { token.0.as_ref() });

        match (entry.is_active.get(), entry.is_head.get()) {
            (true, true) => self.reg_unregister_active_head(token),
            (true, false) => self.reg_unregister_active_tail(token),
            (false, false) => self.reg_unregister_inactive(token),
            _ => unreachable!(),
        }
    }

    fn reg_unregister_active_head(&mut self, token: &mut InnerToken) {
        // Safety: active entry may participate in active_entry_tree tree only
        let mut cursor = unsafe { self.active_entry_tree.cursor_mut_from_ptr(token.0) };
        let elem = unwrap!(cursor.remove());

        // Safety: active entry may participate in clean_up_queue list only
        let clean_up_mark = elem.list_link.is_linked();
        if clean_up_mark {
            unsafe { self.clean_up_queue.cursor_mut_from_ptr(token.0).remove() };
        }

        if self.next_push_ptr == token.0 {
            self.next_push_ptr = core::ptr::null();
            if let Some(entry) = cursor.get()
                && !entry.is_head.get()
            {
                self.next_push_ptr = entry;
            }
        }

        if let Some(next_entry) = cursor.get() {
            let next_was_head = next_entry.is_head.replace(true);
            if (clean_up_mark || next_was_head) && !next_entry.list_link.is_linked() {
                let elem = unsafe { UnsafeRef::from_raw(next_entry) };
                self.free_slot_count += 1;
                self.clean_up_queue.push_back(elem);
                self.filter_update_waker.wake();
            }
        } else {
            self.free_slot_count += 1;
            self.clean_up_last = true;
            self.filter_update_waker.wake();
        }

        unsafe { UnsafeRef::into_raw(elem).drop_in_place() };
    }

    fn reg_unregister_active_tail(&mut self, token: &mut InnerToken) {
        // Safety: active entry may participate in active_entry_tree tree only
        let mut cursor = unsafe { self.active_entry_tree.cursor_mut_from_ptr(token.0) };
        let elem = unwrap!(cursor.remove());

        // Only head entry may participate in the list
        assert!(!elem.list_link.is_linked());

        if self.next_push_ptr == token.0 {
            self.next_push_ptr = core::ptr::null();
            if let Some(entry) = cursor.get()
                && !entry.is_head.get()
            {
                self.next_push_ptr = entry;
            }
        }

        unsafe { UnsafeRef::into_raw(elem).drop_in_place() };
    }

    fn reg_unregister_inactive(&mut self, token: &mut InnerToken) {
        // Safety: inactive entry must participate in inactive_entry_tree
        let mut cursor = unsafe { self.inactive_entry_tree.cursor_mut_from_ptr(token.0) };
        let elem = unwrap!(cursor.remove());

        // Safety: inactive entry must participate in activation_queue
        let mut cursor = unsafe { self.activation_queue.cursor_mut_from_ptr(token.0) };
        unwrap!(cursor.remove());

        unsafe { UnsafeRef::into_raw(elem).drop_in_place() };
    }

    fn with_buffer_mut<T>(
        &mut self,
        token: &mut InnerToken,
        f: impl FnOnce(&mut dyn DynamicRxBuffer) -> T,
    ) -> T {
        let entry = unwrap!(unsafe { token.0.as_ref() });
        let mut buffer_ptr = entry.buffer;
        let buffer = unsafe { buffer_ptr.as_mut() };
        f(buffer)
    }

    fn hub_push_start(&mut self, subject: SubjectId) -> bool {
        self.next_push_ptr = core::ptr::null();

        let cursor = self
            .active_entry_tree
            .lower_bound(Bound::Included(&subject));
        if let Some(entry) = cursor.get() {
            assert!(entry.is_head.get());
            if entry.subject == subject {
                self.next_push_ptr = entry;
            }
        }

        !self.next_push_ptr.is_null()
    }

    fn hub_push_next(&mut self, frame: &Frame, mtu: Mtu) -> bool {
        if self.next_push_ptr.is_null() {
            return false;
        }

        // Safety: next_push_ptr can be part of active_entry_tree only
        let cursor = unsafe { self.active_entry_tree.cursor_from_ptr(self.next_push_ptr) };
        let entry = unwrap!(cursor.get());
        let mut buffer_ptr = entry.buffer;
        let buffer = unsafe { buffer_ptr.as_mut() };
        buffer.push_frame(frame, mtu);

        self.next_push_ptr = core::ptr::null();
        if let Some(next_entry) = cursor.peek_next().get()
            && !next_entry.is_head.get()
        {
            self.next_push_ptr = next_entry;
        }

        !self.next_push_ptr.is_null()
    }

    fn hub_poll_pop_filter_update(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Result<FilterUpdate, CallAgain>> {
        if let Some(val) = self.hub_pop_clean_up_queue() {
            return Poll::Ready(val);
        }
        if let Some(val) = self.hub_pop_clean_up_last() {
            return Poll::Ready(val);
        }
        if let Some(val) = self.hub_pop_registration_queue() {
            return Poll::Ready(val);
        }

        self.filter_update_waker.register(cx.waker());
        Poll::Pending
    }

    fn hub_pop_clean_up_queue(&mut self) -> Option<Result<FilterUpdate, CallAgain>> {
        let entry = self.clean_up_queue.pop_front()?;
        assert!(entry.is_head.get());
        // Once the entry got to the clean up queue, there must have been a unique
        // subject removed before it
        let last_clean_up = unwrap!(prev_subject(entry.subject));

        // Safety: Only active_entry_tree participants may take part in clean_up_queue
        let cursor = unsafe {
            self.active_entry_tree
                .cursor_from_ptr(UnsafeRef::into_raw(entry))
        };
        let first_clean_up = if let Some(prev_entry) = cursor.peek_prev().get() {
            // Once the entry got to the clean up queue, there must have been a unique
            // subject removed after prev_entry
            unwrap!(next_subject(prev_entry.subject))
        } else {
            SubjectId::from_truncating(0)
        };
        assert!(first_clean_up <= last_clean_up);
        let filter_update = FilterUpdate::RemoveSubjectRange([first_clean_up, last_clean_up]);
        Some(Ok(filter_update))
    }

    fn hub_pop_clean_up_last(&mut self) -> Option<Result<FilterUpdate, CallAgain>> {
        if !self.clean_up_last {
            return None;
        }
        self.clean_up_last = false;

        let cursor = self.active_entry_tree.cursor();
        let first_clean_up = if let Some(prev_entry) = cursor.peek_prev().get() {
            // Once the last interval got a clean up flag, there must have been a unique
            // subject removed after the prev_entry
            unwrap!(next_subject(prev_entry.subject))
        } else {
            SubjectId::from_truncating(0)
        };

        let filter_update = FilterUpdate::RemoveSubjectRange([first_clean_up, SubjectId::MAX]);
        Some(Ok(filter_update))
    }

    fn hub_pop_registration_queue(&mut self) -> Option<Result<FilterUpdate, CallAgain>> {
        let elem = self.activation_queue.pop_front()?;
        let subject = elem.subject;

        // Safety: inactive entry must participate in inactive_entry_tree
        let mut cursor = unsafe { self.inactive_entry_tree.cursor_mut_from_ptr(&(*elem)) };
        unwrap!(cursor.remove());

        let mut cursor = self
            .active_entry_tree
            .upper_bound_mut(Bound::Included(&subject));
        let active_subject = cursor
            .get()
            .is_some_and(|prev_entry| prev_entry.subject == subject);

        elem.is_active.set(true);
        elem.is_head.set(!active_subject);
        cursor.insert_after(elem);

        if !active_subject {
            Some(Ok(FilterUpdate::AddSubject(subject)))
        } else {
            Some(Err(CallAgain))
        }
    }
}

// Safety: The object own a unique references to all registered entries.
unsafe impl Send for RegularInner {}

#[derive(Default)]
struct LoopBackInner {
    entry_tree: RBTree<RxTreeAdapter>,
    next_push_ptr: *const Entry,
}

impl LoopBackInner {
    fn reg_register(&mut self, entry_ptr: NonNull<Entry>) -> Result<InnerToken, RegistrationError> {
        let elem = unsafe { UnsafeRef::from_raw(entry_ptr.as_ptr()) };
        let mut cursor = self
            .entry_tree
            .upper_bound_mut(Bound::Included(&elem.subject));
        cursor.insert_after(elem);

        Ok(InnerToken(entry_ptr.as_ptr()))
    }

    fn reg_unregister(&mut self, token: &mut InnerToken) {
        // Safety: a registered entry must participate in entry_tree
        let mut cursor = unsafe { self.entry_tree.cursor_mut_from_ptr(token.0) };
        let elem = unwrap!(cursor.remove());

        if self.next_push_ptr == token.0 {
            self.next_push_ptr = core::ptr::null();
            if let Some(entry) = cursor.get() {
                self.next_push_ptr = entry;
            }
        }

        unsafe { UnsafeRef::into_raw(elem).drop_in_place() };
    }

    fn with_buffer_mut<T>(
        &mut self,
        token: &mut InnerToken,
        f: impl FnOnce(&mut dyn DynamicRxBuffer) -> T,
    ) -> T {
        let entry = unwrap!(unsafe { token.0.as_ref() });
        let mut buffer_ptr = entry.buffer;
        let buffer = unsafe { buffer_ptr.as_mut() };
        f(buffer)
    }

    fn hub_push_start(&mut self, subject: SubjectId) -> bool {
        self.next_push_ptr = core::ptr::null();

        let cursor = self.entry_tree.lower_bound(Bound::Included(&subject));
        if let Some(entry) = cursor.get()
            && entry.subject == subject
        {
            self.next_push_ptr = entry;
        }

        !self.next_push_ptr.is_null()
    }

    fn hub_push_next(&mut self, frame: &Frame, mtu: Mtu) -> bool {
        if self.next_push_ptr.is_null() {
            return false;
        }

        // Safety: next_push_ptr can be part of entry_tree only
        let cursor = unsafe { self.entry_tree.cursor_from_ptr(self.next_push_ptr) };
        let entry = unwrap!(cursor.get());
        let mut buffer_ptr = entry.buffer;
        let buffer = unsafe { buffer_ptr.as_mut() };
        buffer.push_frame(frame, mtu);

        self.next_push_ptr = core::ptr::null();
        if let Some(next_entry) = cursor.peek_next().get()
            && !next_entry.is_head.get()
        {
            self.next_push_ptr = next_entry;
        }

        !self.next_push_ptr.is_null()
    }
}

// Safety: The object own a unique references to all registered entries.
unsafe impl Send for LoopBackInner {}

fn prev_subject(val: SubjectId) -> Option<SubjectId> {
    u16::from(val)
        .checked_sub(1)
        .and_then(|code| SubjectId::try_from(code).ok())
}

fn next_subject(val: SubjectId) -> Option<SubjectId> {
    u16::from(val)
        .checked_add(1)
        .and_then(|code| SubjectId::try_from(code).ok())
}
