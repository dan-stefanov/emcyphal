use emcyphal_driver::frame::Mtu;

/// Allowed frame format for reception and transmission
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum FrameFormat {
    /// Receive and transmit classic frames only
    Classic,
    /// Receive classic and FD frames; transmit FD frames without bit rate switch
    Fd,
    /// Receive classic and FD frames; transmit FD frames with bit rate switch
    FdBrs,
}

impl FrameFormat {
    pub(crate) fn fd(self) -> bool {
        match self {
            FrameFormat::Classic => false,
            FrameFormat::Fd => true,
            FrameFormat::FdBrs => true,
        }
    }

    pub(crate) fn bit_rate_switch(self) -> bool {
        match self {
            FrameFormat::Classic => false,
            FrameFormat::Fd => false,
            FrameFormat::FdBrs => true,
        }
    }

    pub(crate) fn mtu(&self) -> Mtu {
        if self.fd() {
            CAN_FD_MTU
        } else {
            CAN_CLASSIC_MTU
        }
    }
}

/// Adapter configuration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Config {
    /// Allowed frame format for reception and transmission
    pub frame_format: FrameFormat,
}

const CAN_CLASSIC_MTU: Mtu = Mtu::Classic;
const CAN_FD_MTU: Mtu = Mtu::Fd;

impl Default for Config {
    fn default() -> Self {
        Self {
            frame_format: FrameFormat::Classic,
        }
    }
}
