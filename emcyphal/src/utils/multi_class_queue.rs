use crate::utils::DuplexArray;

struct Node {
    next: u8,
    prev: u8,
}

pub const MAX_CAPACITY: usize = u8::MAX as usize + 1;

struct MultiCycleList<const N1: usize, const N2: usize> {
    nodes: DuplexArray<Node, N1, N2>,
}

impl<const N1: usize, const N2: usize> MultiCycleList<N1, N2> {
    const _ASSERT: usize = MAX_CAPACITY - N1 - N2;

    pub fn new() -> Self {
        Self {
            nodes: DuplexArray::from_fn(|i| {
                let node = unwrap!(u8::try_from(i));
                Node {
                    next: node,
                    prev: node,
                }
            }),
        }
    }

    pub fn next(&self, node: u8) -> u8 {
        self.nodes[usize::from(node)].next
    }

    pub fn prev(&self, node: u8) -> u8 {
        self.nodes[usize::from(node)].prev
    }

    pub fn is_bound(&self, node: u8) -> bool {
        self.nodes[usize::from(node)].next != node
    }

    pub fn unbind(&mut self, node: u8) {
        let next = self.nodes[usize::from(node)].next;
        let prev = self.nodes[usize::from(node)].prev;
        self.nodes[usize::from(node)].prev = node;
        self.nodes[usize::from(node)].next = node;
        self.nodes[usize::from(next)].prev = prev;
        self.nodes[usize::from(prev)].next = next;
    }

    pub fn move_after(&mut self, node: u8, prev: u8) {
        self.unbind(node);

        let next = self.nodes[usize::from(prev)].next;
        self.nodes[usize::from(node)].next = next;
        self.nodes[usize::from(node)].prev = prev;
        self.nodes[usize::from(next)].prev = node;
        self.nodes[usize::from(prev)].next = node;
    }

    pub fn move_before(&mut self, node: u8, next: u8) {
        self.unbind(node);

        let prev = self.nodes[usize::from(next)].prev;
        self.nodes[usize::from(node)].next = next;
        self.nodes[usize::from(node)].prev = prev;
        self.nodes[usize::from(next)].prev = node;
        self.nodes[usize::from(prev)].next = node;
    }
}

pub struct MultiClassQueue<const CLASS_COUNT: usize, const ENTRY_COUNT: usize> {
    nodes: MultiCycleList<CLASS_COUNT, ENTRY_COUNT>,
}

impl<const CLASS_COUNT: usize, const ENTRY_COUNT: usize> MultiClassQueue<CLASS_COUNT, ENTRY_COUNT> {
    const _ASSERT: usize = MAX_CAPACITY - CLASS_COUNT - ENTRY_COUNT;
    const ENTRY_OFFSET: u8 = CLASS_COUNT as u8;

    pub fn new() -> Self {
        Self {
            nodes: MultiCycleList::new(),
        }
    }

    pub fn front(&self, class: u8) -> Option<u8> {
        assert!(usize::from(class) < CLASS_COUNT);
        self.nodes.next(class).checked_sub(Self::ENTRY_OFFSET)
    }

    pub fn back(&self, class: u8) -> Option<u8> {
        assert!(usize::from(class) < CLASS_COUNT);
        self.nodes.prev(class).checked_sub(Self::ENTRY_OFFSET)
    }

    pub fn is_empty(&self, class: u8) -> bool {
        assert!(usize::from(class) < CLASS_COUNT);
        self.front(class).is_none()
    }

    pub fn is_front(&self, entry: u8) -> Option<u8> {
        assert!(usize::from(entry) < ENTRY_COUNT);
        let prev = self.nodes.prev(Self::ENTRY_OFFSET + entry);
        if prev < Self::ENTRY_OFFSET {
            Some(prev)
        } else {
            None
        }
    }

    pub fn is_back(&self, entry: u8) -> Option<u8> {
        assert!(usize::from(entry) < ENTRY_COUNT);
        let next = self.nodes.next(Self::ENTRY_OFFSET + entry);
        if next < Self::ENTRY_OFFSET {
            Some(next)
        } else {
            None
        }
    }

    pub fn remove(&mut self, entry: u8) {
        assert!(usize::from(entry) < ENTRY_COUNT);
        self.nodes.unbind(Self::ENTRY_OFFSET + entry);
    }

    pub fn move_front(&mut self, class: u8, entry: u8) {
        assert!(usize::from(class) < CLASS_COUNT);
        assert!(usize::from(entry) < ENTRY_COUNT);
        self.nodes.move_after(Self::ENTRY_OFFSET + entry, class);
    }

    pub fn move_back(&mut self, class: u8, entry: u8) {
        assert!(usize::from(class) < CLASS_COUNT);
        assert!(usize::from(entry) < ENTRY_COUNT);
        self.nodes.move_before(Self::ENTRY_OFFSET + entry, class);
    }
}
