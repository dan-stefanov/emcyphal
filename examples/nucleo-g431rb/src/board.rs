use core::num::NonZero;

pub fn make_peripheral_config() -> embassy_stm32::Config {
    use embassy_stm32::rcc;
    use embassy_stm32::time::Hertz;

    let mut config = embassy_stm32::Config::default();
    config.rcc.hsi = false;
    config.rcc.hse = Some(rcc::Hse {
        freq: Hertz::mhz(24),
        mode: rcc::HseMode::Oscillator,
    });
    config.rcc.pll = Some(rcc::Pll {
        source: rcc::PllSource::HSE,
        prediv: rcc::PllPreDiv::DIV3,
        mul: rcc::PllMul::MUL40, // 320 MHz
        divp: None,
        divq: Some(rcc::PllQDiv::DIV6), // 40 MHz for CAN
        divr: Some(rcc::PllRDiv::DIV2), // 160 MHz sysclock
    });
    config.rcc.sys = rcc::Sysclk::PLL1_R;
    config.rcc.boost = true; // Required for freq > 150MHz
    config.rcc.mux.fdcansel = rcc::mux::Fdcansel::PLL1_Q;
    config
}

pub fn make_can_config_native() -> emcyphal_stm32_native::config::Config {
    use emcyphal_stm32_native::config;
    let mut config = config::Config::default();
    // 1MBps, Sample point location: 0.875
    config.nominal_bit_timing = config::NominalBitTiming {
        prescaler: NonZero::new(5).unwrap(),
        seg1: NonZero::new(6).unwrap(),
        seg2: NonZero::new(1).unwrap(),
        sync_jump_width: NonZero::new(1).unwrap(),
    };
    // 8MBps, Sample point location: 0.9
    config.data_bit_timing = config::DataBitTiming {
        prescaler: NonZero::new(1).unwrap(),
        seg1: NonZero::new(3).unwrap(),
        seg2: NonZero::new(1).unwrap(),
        sync_jump_width: NonZero::new(1).unwrap(),
        // For a typical loop delay TDC is mandatory above 4MBps
        tx_delay_compensation: Some(config::TransceiverDelayCompensation {
            offset: 3, // center of the bit duration
            ..Default::default()
        }),
    };

    config.frame_format = config::FrameFormat::FdBrs;
    config.timestamp_source = config::TimestampSource::System;

    config
}

pub fn make_can_config_embassy() -> embassy_stm32::can::config::FdCanConfig {
    use embassy_stm32::can::config;
    config::FdCanConfig {
        nbtr: config::NominalBitTiming {
            // 1MBps, Sample point location: 0.875
            prescaler: NonZero::new(5).unwrap(),
            seg1: NonZero::new(6).unwrap(),
            seg2: NonZero::new(1).unwrap(),
            sync_jump_width: NonZero::new(1).unwrap(),
        },
        // 4MBps, Sample point location: 0.9
        dbtr: config::DataBitTiming {
            // For a typical loop delay TDC is mandatory above 4MBps.
            // However, as emabassy-stm32 v0.4.0 does not implement it
            transceiver_delay_compensation: false,
            prescaler: NonZero::new(1).unwrap(),
            seg1: NonZero::new(8).unwrap(),
            seg2: NonZero::new(1).unwrap(),
            sync_jump_width: NonZero::new(1).unwrap(),
        },
        automatic_retransmit: true,
        frame_transmit: config::FrameTransmissionConfig::AllowFdCanAndBRS,
        timestamp_source: config::TimestampSource::Prescaler(config::TimestampPrescaler::_1),
        global_filter: config::GlobalFilter {
            handle_standard_frames: config::NonMatchingFilter::Reject,
            handle_extended_frames: config::NonMatchingFilter::Reject,
            reject_remote_standard_frames: true,
            reject_remote_extended_frames: true,
        },
        ..Default::default()
    }
}
