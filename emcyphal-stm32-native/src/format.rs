use emcyphal_core::{NodeId, Priority, ServiceId, SubjectId};

const CAN_ID_MASK: u32 = lsb_mask(29);
const NODE_ID_MASK: u32 = lsb_mask(7);
const SUBJECT_ID_MASK: u32 = lsb_mask(13);
const SERVICE_ID_MASK: u32 = lsb_mask(9);
const PRIORITY_MASK: u32 = lsb_mask(3);

const PRIORITY_OFFSET: u32 = 26;
const SOURCE_OFFSET: u32 = 0;
const MSG_SUBJECT_OFFSET: u32 = 8;
const SRV_DESTINATION_OFFSET: u32 = 7;
const SRV_SERVICE_OFFSET: u32 = 14;

const SERVICE_FLAG: u32 = 1 << 25;
const RES_23_FLAG: u32 = 1 << 23;
const MSG_ANONYMOUS_FLAG: u32 = 1 << 24;
const MSG_RES_7_FLAG: u32 = 1 << 7;
const SRV_REQUEST_FLAG: u32 = 1 << 24;

const MSG_CHECK_MASK: u32 = SERVICE_FLAG | RES_23_FLAG | MSG_RES_7_FLAG;
const MSG_CHECK_VALUE: u32 = 0;
const MSG_IGNORE_MASK: u32 = CAN_ID_MASK
    & !MSG_CHECK_MASK
    & !(PRIORITY_MASK << PRIORITY_OFFSET)
    & !MSG_ANONYMOUS_FLAG
    & !(SUBJECT_ID_MASK << MSG_SUBJECT_OFFSET)
    & !(NODE_ID_MASK << SOURCE_OFFSET);

const SRV_CHECK_MASK: u32 = SERVICE_FLAG | RES_23_FLAG;
const SRV_CHECK_VALUE: u32 = SERVICE_FLAG;
const SRV_IGNORE_MASK: u32 = CAN_ID_MASK
    & !SRV_CHECK_MASK
    & !(PRIORITY_MASK << PRIORITY_OFFSET)
    & !SRV_REQUEST_FLAG
    & !(SERVICE_ID_MASK << SRV_SERVICE_OFFSET)
    & !(NODE_ID_MASK << SRV_DESTINATION_OFFSET)
    & !(NODE_ID_MASK << SOURCE_OFFSET);

pub const MSG_FILTER_MASK: u32 = MSG_CHECK_MASK | (SUBJECT_ID_MASK << MSG_SUBJECT_OFFSET);
pub fn make_msg_filter_pattern(val: Option<SubjectId>) -> u32 {
    if let Some(subject) = val {
        MSG_CHECK_VALUE | u32::from(u16::from(subject)) << MSG_SUBJECT_OFFSET
    } else {
        !MSG_FILTER_MASK & CAN_ID_MASK
    }
}

pub fn decode_msg_filter_pattern(pattern: u32) -> Option<SubjectId> {
    if pattern & !MSG_FILTER_MASK == 0 {
        Some(SubjectId::from_truncating(
            (pattern >> MSG_SUBJECT_OFFSET) as u16,
        ))
    } else {
        None
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct MessageCanId(u32);

impl MessageCanId {
    pub fn new() -> Self {
        Self(MSG_CHECK_VALUE | MSG_IGNORE_MASK)
    }

    pub fn from_can_id(can_id: u32) -> Option<Self> {
        if can_id & MSG_CHECK_MASK == MSG_CHECK_VALUE {
            Some(Self(can_id & CAN_ID_MASK | MSG_IGNORE_MASK))
        } else {
            None
        }
    }

    pub fn priority(self) -> Priority {
        Priority::from_code_truncating((self.0 >> PRIORITY_OFFSET) as u8)
    }

    pub fn set_priority(&mut self, value: Priority) {
        self.0 &= !(PRIORITY_MASK << PRIORITY_OFFSET);
        self.0 |= u32::from(u8::from(value)) << PRIORITY_OFFSET;
    }

    pub fn anonymous(self) -> bool {
        self.0 & MSG_ANONYMOUS_FLAG != 0
    }

    pub fn set_anonymous(&mut self, value: bool) {
        if value {
            self.0 |= MSG_ANONYMOUS_FLAG;
        } else {
            self.0 &= !MSG_ANONYMOUS_FLAG;
        }
    }

    pub fn subject(self) -> SubjectId {
        SubjectId::from_truncating((self.0 >> MSG_SUBJECT_OFFSET) as u16)
    }

    pub fn set_subject(&mut self, value: SubjectId) {
        self.0 &= !(SUBJECT_ID_MASK << MSG_SUBJECT_OFFSET);
        self.0 |= u32::from(u16::from(value)) << MSG_SUBJECT_OFFSET;
    }

    pub fn source(self) -> NodeId {
        NodeId::from_truncating((self.0 >> SOURCE_OFFSET) as u8)
    }

    pub fn set_source(&mut self, value: NodeId) {
        self.0 &= !(NODE_ID_MASK << SOURCE_OFFSET);
        self.0 |= u32::from(u8::from(value)) << SOURCE_OFFSET;
    }
}

impl Default for MessageCanId {
    fn default() -> Self {
        Self::new()
    }
}

impl From<MessageCanId> for u32 {
    fn from(value: MessageCanId) -> Self {
        value.0
    }
}

pub const SRV_FILTER_MASK: u32 = SRV_CHECK_MASK | (NODE_ID_MASK << SRV_DESTINATION_OFFSET);
pub fn make_srv_filter_pattern(dest: Option<NodeId>) -> u32 {
    if let Some(node) = dest {
        SRV_CHECK_VALUE | (u32::from(u8::from(node)) << SRV_DESTINATION_OFFSET)
    } else {
        !SRV_CHECK_MASK & CAN_ID_MASK
    }
}

pub fn decode_srv_filter_pattern(pattern: u32) -> Option<NodeId> {
    if pattern & !SRV_FILTER_MASK == 0 {
        Some(NodeId::from_truncating(
            (pattern >> SRV_DESTINATION_OFFSET) as u8,
        ))
    } else {
        None
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ServiceCanId(u32);

impl ServiceCanId {
    pub fn new() -> Self {
        Self(SRV_CHECK_VALUE | SRV_IGNORE_MASK)
    }

    pub fn from_can_id(can_id: u32) -> Option<Self> {
        if can_id & SRV_CHECK_MASK == SRV_CHECK_VALUE {
            Some(Self(can_id & CAN_ID_MASK | SRV_IGNORE_MASK))
        } else {
            None
        }
    }

    pub fn priority(self) -> Priority {
        Priority::from_code_truncating((self.0 >> PRIORITY_OFFSET) as u8)
    }

    pub fn set_priority(&mut self, value: Priority) {
        self.0 &= !(PRIORITY_MASK << PRIORITY_OFFSET);
        self.0 |= u32::from(u8::from(value)) << PRIORITY_OFFSET;
    }

    pub fn request(self) -> bool {
        self.0 & SRV_REQUEST_FLAG != 0
    }

    pub fn set_request(&mut self, value: bool) {
        if value {
            self.0 |= SRV_REQUEST_FLAG;
        } else {
            self.0 &= !SRV_REQUEST_FLAG;
        }
    }

    pub fn service(self) -> ServiceId {
        ServiceId::from_truncating((self.0 >> SRV_SERVICE_OFFSET) as u16)
    }

    pub fn set_service(&mut self, value: ServiceId) {
        self.0 &= !(SERVICE_ID_MASK << SRV_SERVICE_OFFSET);
        self.0 |= u32::from(u16::from(value)) << SRV_SERVICE_OFFSET;
    }

    pub fn destination(self) -> NodeId {
        NodeId::from_truncating((self.0 >> SRV_DESTINATION_OFFSET) as u8)
    }

    pub fn set_destination(&mut self, value: NodeId) {
        self.0 &= !(NODE_ID_MASK << SRV_DESTINATION_OFFSET);
        self.0 |= u32::from(u8::from(value)) << SRV_DESTINATION_OFFSET;
    }

    pub fn source(self) -> NodeId {
        NodeId::from_truncating((self.0 >> SOURCE_OFFSET) as u8)
    }

    pub fn set_source(&mut self, value: NodeId) {
        self.0 &= !(NODE_ID_MASK << SOURCE_OFFSET);
        self.0 |= u32::from(u8::from(value)) << SOURCE_OFFSET;
    }
}

impl Default for ServiceCanId {
    fn default() -> Self {
        Self::new()
    }
}

impl From<ServiceCanId> for u32 {
    fn from(value: ServiceCanId) -> Self {
        value.0
    }
}

const fn lsb_mask(n: u32) -> u32 {
    if n > 0 {
        u32::MAX >> (u32::BITS - n)
    } else {
        0
    }
}
