use crate::driver::{Info, Instance, SealedInstance};
use crate::message_ram::RegisterBlock;
use crate::raw::{AtomicMethods, Registers};
use embassy_stm32::peripherals as peri;
use stm32_metapac as pac;

macro_rules! impl_fdcan {
    ($peri:ident, $pac_peri:ident, $pac_ram:ident) => {
        impl SealedInstance for peri::$peri {
            fn atomic_methods() -> AtomicMethods {
                AtomicMethods::new(pac::$pac_peri)
            }

            unsafe fn make_registers<'a>() -> Registers<'a> {
                let ptr = pac::$pac_ram.as_ptr() as *mut RegisterBlock;
                let msgram = unsafe { ptr.as_mut().unwrap_unchecked() };
                unsafe { Registers::new(pac::$pac_peri, msgram) }
            }

            fn info() -> &'static Info {
                static INFO: Info = Info::new::<peri::$peri>();
                &INFO
            }
        }

        impl Instance for peri::$peri {}
    };
}

#[cfg(feature = "fdcan1")]
impl_fdcan!(FDCAN1, FDCAN1, FDCANRAM1);

#[cfg(feature = "fdcan2")]
impl_fdcan!(FDCAN2, FDCAN2, FDCANRAM2);

#[cfg(feature = "fdcan3")]
impl_fdcan!(FDCAN3, FDCAN3, FDCANRAM3);
