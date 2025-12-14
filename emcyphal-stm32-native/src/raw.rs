use core::marker::PhantomData;
use emcyphal_core::{NodeId, SubjectId};
use emcyphal_driver::frame;
use stm32_metapac as pac;
use stm32_metapac::can::Fdcan;

use crate::config::{self, FrameFormat};
use crate::format;
use crate::message_ram as ram;
use crate::utils::{TxMailboxIdx, TxMailboxSet};

pub struct Registers<'a> {
    regs: Fdcan,
    msgram: &'a mut ram::RegisterBlock,
}

impl<'a> Registers<'a> {
    pub unsafe fn new(regs: Fdcan, msgram: &'a mut ram::RegisterBlock) -> Self {
        Self { regs, msgram }
    }

    pub fn split(
        self,
    ) -> (
        Control<'a>,
        Filters<'a>,
        RxFifo<'a>,
        TxQueue<'a>,
        TxEventFifo<'a>,
    ) {
        let control = Control {
            regs: self.regs,
            _phantom: PhantomData,
        };
        let filters = Filters {
            elements: &mut self.msgram.filters.flesa,
        };
        let rx_fifo = RxFifo {
            regs: self.regs,
            elements: &mut self.msgram.receive,
        };
        let tx_queue = TxQueue {
            regs: self.regs,
            elements: &mut self.msgram.transmit.tbsa,
        };
        let tx_event_fifo = TxEventFifo {
            regs: self.regs,
            elements: &mut self.msgram.transmit.efsa,
        };
        (control, filters, rx_fifo, tx_queue, tx_event_fifo)
    }

    pub fn configure(&mut self, config: &config::Config) {
        self.start_configuration_change();
        self.set_nominal_bit_timing(&config.nominal_bit_timing);
        self.set_data_bit_timing(&config.data_bit_timing);
        self.set_frame_format(config.frame_format);
        self.set_edge_filtering(config.edge_filtering);
        self.set_protocol_exception_handling(config.protocol_exception_handling);
        self.set_timestamp_source(config.timestamp_source);
        self.set_test_mode(config.test_mode);
        self.set_global_filter_config();
        self.finish_configuration_change();

        // Does not require an CCE
        self.configure_interrupts();
        self.msgram.reset();
        self.init_filters();
    }

    fn start_configuration_change(&self) {
        // Force stop operation
        self.regs.cccr().modify(|w| w.set_init(true));
        while !self.regs.cccr().read().init() {}
        self.regs.cccr().modify(|w| w.set_cce(true));
    }

    fn finish_configuration_change(&self) {
        self.regs.cccr().modify(|w| w.set_cce(false));
    }

    // Could be called in init mode only
    fn set_nominal_bit_timing(&self, val: &config::NominalBitTiming) {
        self.regs.nbtp().write(|w| {
            w.set_nbrp(u16::from(val.prescaler) - 1);
            w.set_ntseg1(u8::from(val.seg1) - 1);
            w.set_ntseg2(u8::from(val.seg2) - 1);
            w.set_nsjw(u8::from(val.sync_jump_width) - 1);
        });
    }

    // Could be called in init mode only
    fn set_data_bit_timing(&self, val: &config::DataBitTiming) {
        self.regs.dbtp().write(|w| {
            w.set_tdc(val.tx_delay_compensation.is_some());
            w.set_dbrp(u8::from(val.prescaler) - 1);
            w.set_dtseg1(u8::from(val.seg1) - 1);
            w.set_dtseg2(u8::from(val.seg2) - 1);
            w.set_dsjw(u8::from(val.sync_jump_width) - 1);
        });

        let tdc = val.tx_delay_compensation.unwrap_or_default();
        self.regs.tdcr().write(|w| {
            w.set_tdco(tdc.offset);
            w.set_tdcf(tdc.filter_window_length);
        })
    }

    // Could be called in init mode only
    fn set_frame_format(&self, frame_format: FrameFormat) {
        self.regs.cccr().modify(|w| {
            w.set_fdoe(frame_format.fd());
            w.set_brse(frame_format.bit_rate_switch());
        });
    }

    // Could be called in init mode only
    fn set_edge_filtering(&self, enabled: bool) {
        self.regs.cccr().modify(|w| w.set_efbi(enabled));
    }

    // Could be called in init mode only
    fn set_protocol_exception_handling(&self, enabled: bool) {
        self.regs.cccr().modify(|w| w.set_pxhd(!enabled));
    }

    // Could be called in init mode only
    fn set_timestamp_source(&self, val: config::TimestampSource) {
        use pac::can::vals::Tss;
        let (tss, tcp) = match val {
            config::TimestampSource::System => (Tss::ZERO, 0),
            config::TimestampSource::Internal(p) => (Tss::INCREMENT, p as u8 - 1),
            config::TimestampSource::ExternalTIM3 => (Tss::EXTERNAL, 0b10),
        };
        self.regs.tscc().write(|w| {
            w.set_tcp(tcp);
            w.set_tss(tss);
        });
    }

    // Could be called in init mode only
    fn set_test_mode(&self, mode: Option<config::TestMode>) {
        if let Some(test) = mode {
            self.regs.cccr().modify(|w| {
                // Enable write access to TEST register
                w.set_test(true);
                // Disable TX pin for internal loopback
                w.set_mon(test == config::TestMode::InternalLoopBack);
            });
            // Enable loop-back mode
            self.regs.test().write(|w| w.set_lbck(true));
        } else {
            self.regs.cccr().modify(|w| {
                // Clear TEST register
                w.set_test(false);
                // Enable TX ping
                w.set_mon(false);
            });
        }
    }

    // Could be called in init mode only
    fn set_global_filter_config(&self) {
        self.regs.rxgfc().modify(|w| {
            use pac::can::vals::{Anfe, Anfs};
            // Do not use standard filters
            w.set_lss(0);
            // Use all extended filters
            w.set_lse(ram::EXTENDED_FILTER_MAX);
            // Reject non-matching standard frames
            w.set_anfs(Anfs::REJECT);
            // Reject non-matching extended frames
            w.set_anfe(Anfe::REJECT);
            // Reject all standard remote frames
            w.set_rrfs(true);
            // Reject all extended remote frames
            w.set_rrfe(true);
        });

        self.regs.xidam().write(|w| {
            // Global AND mask
            // Service filter is not affected, because SRV_MASK is subset of MSG_FILTER_MASK
            const _ASSERT: u32 = 0 - (format::SRV_FILTER_MASK & !format::MSG_FILTER_MASK);
            w.set_eidm(format::MSG_FILTER_MASK);
        })
    }

    fn configure_interrupts(&self) {
        self.regs.ie().write(|w| {
            // Transmission cancellation finished
            w.set_tcfe(true);
            // Transmission completed
            w.set_tce(false);
            // Tx event FIFO new entry
            w.set_tefne(true);

            for i in 0..ram::RX_FIFOS_MAX {
                // Rx FIFO new message
                w.set_rfne(i.into(), true);
            }
        });

        self.regs.ils().write(|w| {
            // Tx FIFO ERROR group on IT1, includes
            //   * Tx event FIFO new entry
            w.set_tferr(true);
            // Status message bit grouping on IT1, includes
            //   * Transmission cancellation finished
            //   * Transmission completed
            w.set_smsg(true);

            for i in 0..ram::RX_FIFOS_MAX {
                // RX FIFO bit group on IT0
                w.set_rxfifo(i.into(), false);
            }
        });

        self.regs.ile().write(|w| {
            // Enable IT0
            w.set_eint0(true);
            // Enable IT1
            w.set_eint1(true);
        })
    }

    // Should be called before the periphery has started
    pub fn init_filters(&self) {
        for i in 0..MESSAGE_FILTER_COUNT {
            set_message_filter(&self.msgram.filters.flesa, i, [None; 2]);
        }
        set_service_filter(&self.msgram.filters.flesa, None);
    }
}

pub struct AtomicMethods {
    regs: Fdcan,
}

impl AtomicMethods {
    pub fn new(regs: Fdcan) -> Self {
        Self { regs }
    }

    pub fn tx_pending(&self) -> TxMailboxSet {
        TxMailboxSet::from_bits_truncating(self.regs.txbrp().read().0 as u8)
    }

    pub fn clear_rx_interrupts(&self) {
        self.regs.ir().write(|w| {
            for i in 0..ram::RX_FIFOS_MAX {
                // Rx FIFO new message
                w.set_rfn(i.into(), true);
            }
        })
    }

    pub fn clear_tx_interrupts(&self) {
        self.regs.ir().write(|w| {
            // Transmission cancellation finished
            w.set_tcf(true);
            // Transmission completed
            w.set_tc(false);
            // Tx event FIFO new entry
            w.set_tefn(true);
        })
    }
}

pub struct Control<'a> {
    regs: Fdcan,
    _phantom: PhantomData<&'a mut ()>,
}

impl<'a> Control<'a> {
    pub fn start(&mut self) {
        self.regs.cccr().modify(|w| w.set_init(false));
        while self.regs.cccr().read().init() {}
    }

    pub fn stop(&mut self) {
        self.regs.cccr().modify(|w| w.set_init(true));
        while !self.regs.cccr().read().init() {}
    }

    pub fn internal_timestamp_counter(&self) -> u16 {
        self.regs.tscv().read().0 as u16
    }

    pub fn external_timestamp_counter(&self) -> u16 {
        // FDCAN is hardwired to TIM3
        pac::TIM3.cnt().read().0 as u16
    }
}

pub const MESSAGE_FILTER_COUNT: usize = ram::EXTENDED_FILTER_MAX as usize - 1;
const MESSAGE_FILTER_OFFSET: usize = 0;
const SERVICE_FILTER_OFFSET: usize = MESSAGE_FILTER_OFFSET + MESSAGE_FILTER_COUNT;

pub struct Filters<'a> {
    elements: &'a mut [ram::ExtendedFilter; ram::EXTENDED_FILTER_MAX as usize],
}

impl<'a> Filters<'a> {
    pub fn set_message_filter(&mut self, n: usize, val: [Option<SubjectId>; 2]) {
        set_message_filter(self.elements, n, val);
    }

    pub fn get_message_filter(&mut self, n: usize) -> [Option<SubjectId>; 2] {
        get_message_filter(self.elements, n)
    }

    pub fn set_service_filter(&mut self, dest: Option<NodeId>) {
        set_service_filter(self.elements, dest);
    }

    pub fn get_service_filter(&mut self) -> Option<NodeId> {
        get_service_filter(self.elements)
    }
}

type FilterBlock = [ram::ExtendedFilter; ram::EXTENDED_FILTER_MAX as usize];

fn set_message_filter(elements: &FilterBlock, n: usize, val: [Option<SubjectId>; 2]) {
    use ram::enums::{FilterElementConfig, FilterType};

    assert!(n < MESSAGE_FILTER_COUNT);
    let elem = &elements[MESSAGE_FILTER_OFFSET + n];
    elem.write(|w| {
        let patterns = val.map(format::make_msg_filter_pattern);

        w.eft().set_filter_type(FilterType::DualIdFilter);
        unsafe { w.efid1().bits(patterns[0]) };
        unsafe { w.efid2().bits(patterns[1]) };
        w.efec()
            .set_filter_element_config(FilterElementConfig::StoreInFifo0);
        w
    });
}

// The set_message_filter should be applied first
fn get_message_filter(elements: &FilterBlock, n: usize) -> [Option<SubjectId>; 2] {
    use ram::enums::FilterType;
    assert!(n < MESSAGE_FILTER_COUNT);
    let elem = &elements[MESSAGE_FILTER_OFFSET + n];
    let filter = elem.read();
    assert_eq!(filter.eft().to_filter_type(), FilterType::DualIdFilter);
    let patterns = [filter.efid1().bits(), filter.efid2().bits()];
    patterns.map(format::decode_msg_filter_pattern)
}

fn set_service_filter(elements: &FilterBlock, dest: Option<NodeId>) {
    use ram::enums::{FilterElementConfig, FilterType};

    let mask: u32 = format::SRV_FILTER_MASK;
    let pattern = format::make_srv_filter_pattern(dest);

    let elem = &elements[SERVICE_FILTER_OFFSET];
    elem.write(|w| {
        w.eft().set_filter_type(FilterType::ClassicFilter);
        unsafe { w.efid1().bits(pattern) };
        unsafe { w.efid2().bits(mask) };
        w.efec()
            .set_filter_element_config(FilterElementConfig::StoreInFifo1);
        w
    });
}

// The set_service_filter should be applied first
fn get_service_filter(elements: &FilterBlock) -> Option<NodeId> {
    use ram::enums::FilterType;
    let elem = &elements[SERVICE_FILTER_OFFSET];
    let filter = elem.read();
    assert_eq!(filter.eft().to_filter_type(), FilterType::ClassicFilter);
    format::decode_srv_filter_pattern(filter.efid1().bits)
}

pub struct RawFrame {
    pub header: frame::Header,
    pub data: frame::Data,
    pub timestamp: u16,
}

pub struct RxFifo<'a> {
    regs: Fdcan,
    elements: &'a mut [ram::Receive; ram::RX_FIFOS_MAX as usize],
}

impl<'a> RxFifo<'a> {
    pub fn get_index(&self, n: usize) -> Option<u8> {
        let rxfs = self.regs.rxfs(n).read();
        let non_empty = rxfs.ffl() > 0;
        non_empty.then_some(rxfs.fgi())
    }

    pub fn acknowledge(&self, n: usize, idx: u8) {
        self.regs.rxfa(n).write(|w| w.set_fai(idx));
    }

    pub fn pop(&self, n: usize) -> Option<RawFrame> {
        while let Some(idx) = self.get_index(n) {
            let frame = load_frame(&self.elements[n].fxsa[usize::from(idx)]);
            self.acknowledge(n, idx);
            if frame.is_some() {
                return frame;
            }
        }
        None
    }
}

fn load_msg_frame_header(can_id: u32) -> Option<frame::Header> {
    let msg_id = format::MessageCanId::from_can_id(can_id)?;
    Some(frame::Header {
        priority: msg_id.priority(),
        data_spec: frame::DataSpecifier::Message(msg_id.subject()),
        source: (!msg_id.anonymous()).then_some(msg_id.source()),
        destination: None,
    })
}

fn load_srv_frame_header(can_id: u32) -> Option<frame::Header> {
    let srv_id = format::ServiceCanId::from_can_id(can_id)?;
    Some(frame::Header {
        priority: srv_id.priority(),
        data_spec: if srv_id.request() {
            frame::DataSpecifier::Request(srv_id.service())
        } else {
            frame::DataSpecifier::Response(srv_id.service())
        },
        source: Some(srv_id.source()),
        destination: Some(srv_id.destination()),
    })
}

pub fn load_frame(elem: &ram::RxFifoElement) -> Option<RawFrame> {
    let raw_header = elem.header.read();
    assert!(
        raw_header.xtd().is_extended_id(),
        "Only extended frames should pass the filter"
    );
    assert!(
        raw_header.rtr().is_transmit_data_frame(),
        "Only data frame should pass the filter"
    );

    let can_id = raw_header.id().bits();
    let timestamp = raw_header.rxts().bits();

    let header = if usize::from(raw_header.fidx().bits()) != SERVICE_FILTER_OFFSET {
        load_msg_frame_header(can_id)?
    } else {
        load_srv_frame_header(can_id)?
    };

    let data_length = unwrap!(frame::DataLength::new(
        raw_header.to_data_length().len().into()
    ));
    let mut data = frame::Data::new_zeros(data_length);
    for (i, dst) in data.chunks_mut(4).enumerate() {
        let bytes = elem.data[i].read().to_le_bytes();
        dst.copy_from_slice(&bytes[..dst.len()]);
    }

    Some(RawFrame {
        header,
        data,
        timestamp,
    })
}

pub struct TxQueue<'a> {
    regs: Fdcan,
    elements: &'a mut [ram::TxBufferElement; ram::TX_FIFO_MAX as usize],
}

impl<'a> TxQueue<'a> {
    pub fn pending(&self) -> TxMailboxSet {
        TxMailboxSet::from_bits_truncating(self.regs.txbrp().read().0 as u8)
    }

    #[allow(dead_code)]
    pub fn completed(&self) -> TxMailboxSet {
        TxMailboxSet::from_bits_truncating(self.regs.txbto().read().0 as u8)
    }

    pub fn add(
        &mut self,
        mailbox_idx: TxMailboxIdx,
        frame: &frame::Frame,
        frame_format: FrameFormat,
        marker: u8,
    ) -> Result<(), ()> {
        if self.pending().contains(mailbox_idx) {
            return Err(());
        }

        store_tx_buffer(
            &self.elements[usize::from(u8::from(mailbox_idx))],
            frame,
            frame_format,
            marker,
        );
        self.add_request(TxMailboxSet::new_eq(mailbox_idx));
        Ok(())
    }

    pub fn cancel(&mut self, mailboxes: TxMailboxSet) {
        self.regs
            .txbcr()
            .write(|w| w.0 = mailboxes.into_bits() as u32);
    }

    pub fn enable_interrupt(&mut self, mailboxes: TxMailboxSet) {
        self.regs
            .txbtie()
            .write(|w| w.0 = mailboxes.into_bits() as u32)
    }

    fn add_request(&mut self, mailboxes: TxMailboxSet) {
        self.regs
            .txbar()
            .write(|w| w.0 = mailboxes.into_bits() as u32);
    }
}

#[derive(Debug)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
struct InvalidFrameHeader();

fn make_can_id(header: &frame::Header) -> Result<u32, InvalidFrameHeader> {
    // anonymous frame transmission is not supported
    match (header.data_spec, header.destination, header.source) {
        (frame::DataSpecifier::Message(subject), None, Some(src)) => {
            let mut can_id = format::MessageCanId::new();
            can_id.set_priority(header.priority);
            can_id.set_anonymous(false);
            can_id.set_subject(subject);
            can_id.set_source(src);
            Ok(can_id.into())
        }
        (frame::DataSpecifier::Request(service), Some(dst), Some(src)) => {
            let mut can_id = format::ServiceCanId::new();
            can_id.set_priority(header.priority);
            can_id.set_request(true);
            can_id.set_service(service);
            can_id.set_destination(dst);
            can_id.set_source(src);
            Ok(can_id.into())
        }
        (frame::DataSpecifier::Response(service), Some(dst), Some(src)) => {
            let mut can_id = format::ServiceCanId::new();
            can_id.set_priority(header.priority);
            can_id.set_request(false);
            can_id.set_service(service);
            can_id.set_destination(dst);
            can_id.set_source(src);
            Ok(can_id.into())
        }
        _ => Err(InvalidFrameHeader()),
    }
}

fn make_dlc(value: frame::DataLength) -> u8 {
    let len = usize::from(value);
    match len {
        0..=8 => len as u8,
        12 => 9,
        16 => 10,
        20 => 11,
        24 => 12,
        32 => 13,
        48 => 14,
        64 => 15,
        _ => unreachable!(),
    }
}

pub fn store_tx_buffer(
    elem: &ram::TxBufferElement,
    frame: &frame::Frame,
    frame_format: FrameFormat,
    marker: u8,
) {
    let can_id = unwrap!(make_can_id(&frame.header));

    assert!(frame_format.fd() || frame.data.len() <= 8);
    let dlc = make_dlc(frame.data.length());

    elem.header.write(|w| {
        // Do not force ESI bit
        w.esi()
            .set_error_indicator(ram::enums::ErrorStateIndicator::ErrorActive);
        w.xtd().set_id_type(ram::enums::IdType::ExtendedId);
        w.rtr()
            .set_rtr(ram::enums::RemoteTransmissionRequest::TransmitDataFrame);
        unsafe { w.id().bits(can_id) };
        unsafe { w.mm().bits(marker) };
        w.efc().set_event_control(ram::enums::EventControl::Store);
        w.fdf().bit(frame_format.fd());
        w.brs().bit(frame_format.bit_rate_switch());
        unsafe { w.dlc().bits(dlc) };
        w
    });

    for (i, src) in frame.data.chunks(4).enumerate() {
        let mut bytes = [0u8; 4];
        bytes[..src.len()].copy_from_slice(src);
        unsafe { elem.data[i].write(u32::from_le_bytes(bytes)) };
    }
}

pub struct TxEvent {
    pub marker: u8,
    pub timestamp: u16,
}

pub struct TxEventFifo<'a> {
    regs: Fdcan,
    elements: &'a mut [ram::TxEventElement; ram::TX_EVENT_MAX as usize],
}

impl<'a> TxEventFifo<'a> {
    fn get_index(&self) -> Option<u8> {
        let txefs = self.regs.txefs().read();
        let non_empty = txefs.effl() > 0;
        non_empty.then_some(txefs.efgi())
    }

    fn acknowledge(&self, idx: u8) {
        self.regs.txefa().write(|w| w.set_efai(idx));
    }

    pub fn pop(&mut self) -> Option<TxEvent> {
        if let Some(idx) = self.get_index() {
            let event = load_tx_event(&self.elements[usize::from(idx)]);
            self.acknowledge(idx);
            Some(event)
        } else {
            None
        }
    }
}

pub fn load_tx_event(elem: &ram::TxEventElement) -> TxEvent {
    let r = elem.read();
    TxEvent {
        marker: r.mm().bits(),
        timestamp: r.txts().bits(),
    }
}
