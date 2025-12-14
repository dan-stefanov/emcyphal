use core::num::NonZeroU8;
use core::num::NonZeroU16;
use emcyphal_driver::frame::Mtu;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum TimestampPrescaler {
    /// 1
    _1 = 1,
    /// 2
    _2 = 2,
    /// 3
    _3 = 3,
    /// 4
    _4 = 4,
    /// 5
    _5 = 5,
    /// 6
    _6 = 6,
    /// 7
    _7 = 7,
    /// 8
    Classic = 8,
    /// 9
    _9 = 9,
    /// 10
    _10 = 10,
    /// 11
    _11 = 11,
    /// 12
    _12 = 12,
    /// 13
    _13 = 13,
    /// 14
    _14 = 14,
    /// 15
    _15 = 15,
    /// 16
    _16 = 16,
}

/// Allowed frame format for reception and transmission
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum FrameFormat {
    /// Receive and transmit classic frames only
    Classic,
    /// Receive classic and FD frames, transmit FD frames without bit rate switch
    Fd,
    /// Receive classic and FD frames, transmit FD frames with bit rate switch
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

    pub(crate) fn mtu(self) -> Mtu {
        if self.fd() { Mtu::Fd } else { Mtu::Classic }
    }
}

/// Bit timing during arbitration phase and data phase without bit rate switch (BRS)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct NominalBitTiming {
    /// Prescaler for the oscillator frequency. The bit time is built from multiples of this quantum.
    /// Valid range: 1 to 512.
    pub prescaler: NonZeroU16,
    /// Time segment 1 (includes propagation and phase segments).
    /// Valid range: 1 to 128.
    pub seg1: NonZeroU8,
    /// Time segment 2 (phase segment 2).
    /// Valid range: 1 to 255.
    pub seg2: NonZeroU8,
    /// Synchronization jump width for clock tolerance.
    /// Valid range: 1 to 128.
    pub sync_jump_width: NonZeroU8,
}

impl Default for NominalBitTiming {
    #[inline]
    fn default() -> Self {
        // Kernel clock: 8 MHz, bit rate: 500 kbit/s. NBTP register value: 0x0600_0A03
        Self {
            prescaler: unwrap!(NonZeroU16::new(1)),
            seg1: unwrap!(NonZeroU8::new(11)),
            seg2: unwrap!(NonZeroU8::new(4)),
            sync_jump_width: unwrap!(NonZeroU8::new(4)),
        }
    }
}

/// Transceiver delay compensation during data phase with bit rate switch (BRS).
///
/// During arbitration, the transmitter checks the bus state at the nominal sampling point
/// (after nominal-TSEG1) to detect collisions. This sampling point may be restrictive for
/// the data phase, as it limits transceiver loop delay to be less than data-TSEG1.
/// This feature enables automatic loop delay measurement at the start of the data phase,
/// positioning a second sampling point (SSP) at measured_delay + offset.
/// The SSP is clamped at 127 Tq and should not exceed 6 data bit times.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct TransceiverDelayCompensation {
    /// SSP offset from the measured delay, in time quanta (Tq). Typically half the data bit time.
    /// Valid range: 0 to 127.
    pub offset: u8,
    /// Duration (in Tq) to block dominant bus level detection and avoid locking to early glitches.
    /// Valid range: 0 to 127.
    pub filter_window_length: u8,
}

/// Bit timing during data phase with bit rate switch (BRS)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct DataBitTiming {
    /// Prescaler for the oscillator frequency during the data phase. The bit time is built from multiples of this quantum.
    /// Valid range: 1 to 31.
    pub prescaler: NonZeroU8,
    /// Time segment 1 (includes propagation and phase segments).
    /// Valid range: 1 to 31.
    pub seg1: NonZeroU8,
    /// Time segment 2 (phase segment 2).
    /// Valid range: 1 to 15.
    pub seg2: NonZeroU8,
    /// Synchronization jump width for clock tolerance.
    /// Valid range: 1 to 15.
    pub sync_jump_width: NonZeroU8,
    /// Enable and configure transceiver delay compensation. Recommended for data bit rates above 1 Mbps.
    pub tx_delay_compensation: Option<TransceiverDelayCompensation>,
}

impl Default for DataBitTiming {
    #[inline]
    fn default() -> Self {
        // Kernel clock: 8 MHz, Bit rate: 500 kbit/s. DBTP register value of 0x0000_0A33
        Self {
            prescaler: unwrap!(NonZeroU8::new(1)),
            seg1: unwrap!(NonZeroU8::new(11)),
            seg2: unwrap!(NonZeroU8::new(4)),
            sync_jump_width: unwrap!(NonZeroU8::new(4)),
            tx_delay_compensation: None,
        }
    }
}

/// Timestamp source for received and transmitted frames
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum TimestampSource {
    /// Timestamp is set from the system clock in the runner task
    System,
    /// Internal free-running counter with a prescaler applied to the bit frequency.
    /// The resulting frequency should match the system clock tick rate.
    /// Not compatible with CAN-FD format because bit rate switch affects the counter.
    Internal(TimestampPrescaler),
    /// External free-running TIM3. The timer frequency should match the system clock tick rate.
    /// Can be shared across multiple FDCAN peripherals and used as the system clock driver.
    /// Supports CAN-FD format with bit rate switch.
    ExternalTIM3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum TestMode {
    /// Connects TX to RX internally and allows reception of own frames.
    /// Transmitter ignores acknowledgement errors.
    /// RX pin is disconnected from the peripheral. TX pin is connected to transmitter output.
    ExternalLoopBack,
    /// Connects TX to RX internally and allows reception of own frames.
    /// Transmitter ignores acknowledgement errors.
    /// RX pin is disconnected from the peripheral. TX pin is held recessive.
    InternalLoopBack,
}

/// Driver config struct
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Config {
    /// Bit timing during arbitration and data phase without bitrate switch (BRS)
    pub nominal_bit_timing: NominalBitTiming,
    /// Bit timing during data phase with bitrate switch (BRS). Ignored for Classic frame format.
    pub data_bit_timing: DataBitTiming,
    /// Allowed frame format for reception and transmission
    pub frame_format: FrameFormat,
    /// Edge filtering during bus integration. When enabled, two consecutive dominant Tq are
    /// required to detect an edge for hard synchronization.
    pub edge_filtering: bool,
    /// A CAN-FD frame has a bit reserved for future extension.
    /// Skip unsupported frames when handling is enabled. Send an error frame otherwise.
    pub protocol_exception_handling: bool,
    pub timestamp_source: TimestampSource,
    pub test_mode: Option<TestMode>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            nominal_bit_timing: Default::default(),
            data_bit_timing: Default::default(),
            frame_format: FrameFormat::Classic,
            edge_filtering: false,
            protocol_exception_handling: true,
            timestamp_source: TimestampSource::System,
            test_mode: Default::default(),
        }
    }
}
