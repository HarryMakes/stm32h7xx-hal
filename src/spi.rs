//! Serial Peripheral Interface (SPI) bus
//!
//! This module implements the [embedded-hal](embedded-hal) traits for
//! master mode SPI.
//!
//! # Usage
//!
//! In the simplest case, SPI can be initialised from the device
//! peripheral and the GPIO pins.
//!
//! ```
//! use stm32h7xx_hal::spi;
//!
//! let dp = ...;                   // Device peripherals
//! let (sck, miso, mosi) = ...;    // GPIO pins
//!
//! let spi = dp.SPI1.spi((sck, miso, mosi), spi::MODE_0, 1.mhz(), &mut ccdr);
//! ```
//!
//! The GPIO pins should be supplied as a
//! tuple in the following order:
//!
//! - Serial Clock (SCK)
//! - Master In Slave Out (MISO)
//! - Master Out Slave In (MOSI)
//!
//! If one of the pins is not required, explicitly pass one of the
//! filler types instead:
//!
//! ```
//! let spi = dp.SPI1.spi((sck, spi::NoMiso, mosi), spi::MODE_0, 1.mhz(), &mut ccdr);
//! ```
//!
//! ## Clocks
//!
//! The bitrate calculation is based upon the clock currently assigned
//! in the RCC CCIP register. The default assignments are:
//!
//! - SPI1, SPI2, SPI3: __PLL1 Q CK__
//! - SPI4, SPI5: __APB__
//! - SPI6: __PCLK4__
//!
//! [embedded_hal]: https://docs.rs/embedded-hal/0.2.3/embedded_hal/spi/index.html

use crate::hal;
pub use crate::hal::spi::{
    Mode, Phase, Polarity, MODE_0, MODE_1, MODE_2, MODE_3,
};
use crate::stm32::rcc::{d2ccip1r, d3ccipr};
use crate::stm32::spi1::cfg1::MBR_A as MBR;
use core::ptr;
use nb;
use stm32h7::Variant::Val;
use core::convert::From;

use crate::stm32::{SPI1, SPI2, SPI3, SPI4, SPI5, SPI6};

use crate::gpio::gpioa::{PA12, PA5, PA6, PA7, PA9};
use crate::gpio::gpiob::{PB10, PB13, PB14, PB15, PB2, PB3, PB4, PB5};
use crate::gpio::gpioc::{PC1, PC10, PC11, PC12, PC2, PC3};
use crate::gpio::gpiod::{PD3, PD6, PD7};
use crate::gpio::gpioe::{PE12, PE13, PE14, PE2, PE5, PE6};
use crate::gpio::gpiof::{PF11, PF7, PF8, PF9};
use crate::gpio::gpiog::{PG11, PG12, PG13, PG14, PG9};
use crate::gpio::gpioh::{PH6, PH7};
use crate::gpio::gpioi::{PI1, PI2, PI3};
use crate::gpio::gpioj::{PJ10, PJ11};
use crate::gpio::gpiok::PK0;

use crate::gpio::{Alternate, AF5, AF6, AF7, AF8};

use crate::rcc::Ccdr;
use crate::time::Hertz;

/// SPI error
#[derive(Debug)]
pub enum Error {
    /// Overrun occurred
    Overrun,
    /// Mode fault occurred
    ModeFault,
    /// CRC error
    Crc,
    #[doc(hidden)]
    _Extensible,
}

pub trait Pins<SPI> {}
pub trait PinSck<SPI> {}
pub trait PinMiso<SPI> {}
pub trait PinMosi<SPI> {}

impl<SPI, SCK, MISO, MOSI> Pins<SPI> for (SCK, MISO, MOSI)
where
    SCK: PinSck<SPI>,
    MISO: PinMiso<SPI>,
    MOSI: PinMosi<SPI>,
{
}

#[derive(Debug, Copy, Clone)]
pub enum CommunicationMode {
    FullDuplex = 0,
    Transmitter = 1,
    Receiver = 2,
    HalfDuplex = 3,
}

/// A structure for specifying SPI configuration.
///
/// This structure uses builder semantics to generate the configuration.
///
/// `Example`
/// ```
/// use embedded_hal::spi::Mode;
///
/// let config = Config::new(Mode::MODE_0)
///     .frame_size(8)
/// ```
#[derive(Copy, Clone)]
pub struct Config {
    mode: Mode,
    swap_miso_mosi: bool,
    cs_delay: f32,
    frame_size: u8,
    managed_cs: bool,
    communication_mode: CommunicationMode,
    t_size: u16,
}

impl Config {
    /// Create a default configuration for the SPI interface.
    ///
    /// Arguments:
    /// * `mode` - The SPI mode to configure.
    pub fn new(mode: Mode) -> Self {
        Config {
            mode: mode,
            swap_miso_mosi: false,
            cs_delay: 0.0,
            frame_size: 8_u8,
            managed_cs: false,
            communication_mode: CommunicationMode::FullDuplex,
            t_size: 0u16,
        }
    }

    /// Specify that the SPI MISO/MOSI lines are swapped.
    ///
    /// Note:
    /// * This function updates the HAL peripheral to treat the pin provided in the MISO parameter
    /// as the MOSI pin and the pin provided in the MOSI parameter as the MISO pin.
    pub fn swap_mosi_miso(mut self) -> Self {
        self.swap_miso_mosi = true;
        self
    }

    /// Specify a delay between CS assertion and the beginning of the SPI transaction.
    ///
    /// Note:
    /// * This function introduces a delay on SCK from the initiation of the transaction. The delay
    /// is specified as a number of SCK cycles, so the actual delay may vary.
    ///
    /// Arguments:
    /// * `delay` - The delay between CS assertion and the start of the transaction in seconds.
    /// register for the output pin.
    pub fn cs_delay(mut self, delay: f32) -> Self {
        self.cs_delay = delay;
        self
    }

    /// Specify the SPI transaction size.
    ///
    /// Arguments:
    /// * `frame_size` - The size of each SPI transaction in bits.
    pub fn frame_size(mut self, frame_size: u8) -> Self {
        self.frame_size = frame_size;
        self
    }

    /// CS pin is automatically managed by the SPI peripheral.
    pub fn manage_cs(mut self) -> Self {
        self.managed_cs = true;
        self
    }

    pub fn communication_mode(mut self, comms: CommunicationMode) -> Self {
        self.communication_mode = comms;
        self
    }

    pub fn transfer_size(mut self, size: u16) -> Self {
        self.t_size = size;
        self
    }
}

impl From<Mode> for Config {
    fn from(mode: Mode) -> Self {
        Self::new(mode)
    }
}

/// A filler type for when the SCK pin is unnecessary
pub struct NoSck;
/// A filler type for when the Miso pin is unnecessary
pub struct NoMiso;
/// A filler type for when the Mosi pin is unnecessary
pub struct NoMosi;

macro_rules! pins {
    ($($SPIX:ty: SCK: [$($SCK:ty),*] MISO: [$($MISO:ty),*] MOSI: [$($MOSI:ty),*])+) => {
        $(
            $(
                impl PinSck<$SPIX> for $SCK {}
            )*
            $(
                impl PinMiso<$SPIX> for $MISO {}
            )*
            $(
                impl PinMosi<$SPIX> for $MOSI {}
            )*
        )+
    }
}

pins! {
    SPI1:
        SCK: [
            NoSck,
            PA5<Alternate<AF5>>,
            PB3<Alternate<AF5>>,
            PG11<Alternate<AF5>>
        ]
        MISO: [
            NoMiso,
            PA6<Alternate<AF5>>,
            PB4<Alternate<AF5>>,
            PG9<Alternate<AF5>>
        ]
        MOSI: [
            NoMosi,
            PA7<Alternate<AF5>>,
            PB5<Alternate<AF5>>,
            PD7<Alternate<AF5>>
        ]
    SPI2:
        SCK: [
            NoSck,
            PA9<Alternate<AF5>>,
            PA12<Alternate<AF5>>,
            PB10<Alternate<AF5>>,
            PB13<Alternate<AF5>>,
            PD3<Alternate<AF5>>,
            PI1<Alternate<AF5>>
        ]
        MISO: [
            NoMiso,
            PB14<Alternate<AF5>>,
            PC2<Alternate<AF5>>,
            PI2<Alternate<AF5>>
        ]
        MOSI: [
            NoMosi,
            PB15<Alternate<AF5>>,
            PC1<Alternate<AF5>>,
            PC3<Alternate<AF5>>,
            PI3<Alternate<AF5>>
        ]
    SPI3:
        SCK: [
            NoSck,
            PB3<Alternate<AF6>>,
            PC10<Alternate<AF6>>
        ]
        MISO: [
            NoMiso,
            PB4<Alternate<AF6>>,
            PC11<Alternate<AF6>>
        ]
        MOSI: [
            NoMosi,
            PB2<Alternate<AF7>>,
            PB5<Alternate<AF7>>,
            PC12<Alternate<AF6>>,
            PD6<Alternate<AF5>>
        ]
    SPI4:
        SCK: [
            NoSck,
            PE2<Alternate<AF5>>,
            PE12<Alternate<AF5>>
        ]
        MISO: [
            NoMiso,
            PE5<Alternate<AF5>>,
            PE13<Alternate<AF5>>
        ]
        MOSI: [
            NoMosi,
            PE6<Alternate<AF5>>,
            PE14<Alternate<AF5>>
        ]
    SPI5:
        SCK: [
            NoSck,
            PF7<Alternate<AF5>>,
            PH6<Alternate<AF5>>,
            PK0<Alternate<AF5>>
        ]
        MISO: [
            NoMiso,
            PF8<Alternate<AF5>>,
            PH7<Alternate<AF5>>,
            PJ11<Alternate<AF5>>
        ]
        MOSI: [
            NoMosi,
            PF9<Alternate<AF5>>,
            PF11<Alternate<AF5>>,
            PJ10<Alternate<AF5>>
        ]
    SPI6:
        SCK: [
            NoSck,
            PA5<Alternate<AF8>>,
            PB3<Alternate<AF8>>,
            PG13<Alternate<AF5>>
        ]
        MISO: [
            NoMiso,
            PA6<Alternate<AF8>>,
            PB4<Alternate<AF8>>,
            PG12<Alternate<AF5>>
        ]
        MOSI: [
            NoMosi,
            PA7<Alternate<AF8>>,
            PB5<Alternate<AF8>>,
            PG14<Alternate<AF5>>
        ]
}

/// Interrupt events
pub enum Event {
    /// New data has been received
    Rxp,
    /// Data can be sent
    Txp,
    /// An error occurred
    Error,

    /// A transfer has completed.
    Eot,
}

#[derive(Debug)]
pub struct Spi<SPI> {
    pub spi: SPI,
}

pub trait SpiExt<SPI>: Sized {
    fn spi<PINS, T, CONFIG>(
        self,
        _pins: PINS,
        config: CONFIG,
        freq: T,
        ccdr: &Ccdr,
    ) -> Spi<SPI>
    where
        PINS: Pins<SPI>,
        T: Into<Hertz>,
        CONFIG: Into<Config>;

    fn spi_unchecked<T, CONFIG>(
        self,
        config: CONFIG,
        freq: T,
        ccdr: &Ccdr,
    ) -> Spi<SPI>
    where
        T: Into<Hertz>,
        CONFIG: Into<Config>;
}

macro_rules! spi {
	($($SPIX:ident: ($spiX:ident, $apbXenr:ident, $spiXen:ident,
                     $pclkX:ident) => ($($TY:ident),+),)+) => {
	    $(
            impl Spi<$SPIX> {
                pub fn $spiX<T, CONFIG>(
                    spi: $SPIX,
                    config: CONFIG,
                    freq: T,
                    ccdr: &Ccdr,
                ) -> Self
                where
                    T: Into<Hertz>,
                    CONFIG: Into<Config>,
                {
                    // Enable clock for SPI
                    ccdr.rb.$apbXenr.modify(|_, w| w.$spiXen().enabled());

                    // Disable SS output
                    spi.cfg2.write(|w| w.ssoe().disabled());

                    let config: Config = config.into();

                    let spi_freq = freq.into().0;
	                let spi_ker_ck = match Self::kernel_clk(ccdr) {
                        Some(ker_hz) => ker_hz.0,
                        _ => panic!("$SPIX kernel clock not running!")
                    };
                    let mbr = match spi_ker_ck / spi_freq {
                        0 => unreachable!(),
                        1..=2 => MBR::DIV2,
                        3..=5 => MBR::DIV4,
                        6..=11 => MBR::DIV8,
                        12..=23 => MBR::DIV16,
                        24..=47 => MBR::DIV32,
                        48..=95 => MBR::DIV64,
                        96..=191 => MBR::DIV128,
                        _ => MBR::DIV256,
                    };
                    spi.cfg1.modify(|_, w| {
                        w.mbr()
                            .variant(mbr) // master baud rate
                            .dsize()
                            .bits(config.frame_size - 1)
                    });

                    // Each transaction is 1 word in size.
                    spi.cr2.write(|w| w.tsize().bits(config.t_size));

                    // ssi: select slave = master mode
                    spi.cr1.write(|w| w.ssi().slave_not_selected());

                    // Calculate the CS->transaction cycle delay bits.
                    let cycle_delay: u8 = {
                        let mut delay: u32 = (config.cs_delay * spi_freq as f32) as u32;

                        // If the cs-delay is specified as non-zero, add 1 to the delay cycles
                        // before truncation to an integer to ensure that we have at least as
                        // many cycles as required.
                        if config.cs_delay > 0.0_f32 {
                            delay = delay + 1;
                        }

                        if delay > 0xF {
                            delay = 0xF;
                        }

                        delay as u8
                    };

                    // mstr: master configuration
                    // lsbfrst: MSB first
                    // ssm: enable software slave management (NSS pin
                    // free for other uses)
                    // comm: full-duplex
                    spi.cfg2.write(|w| {
                        w.cpha()
                            .bit(config.mode.phase == Phase::CaptureOnSecondTransition)
                         .cpol()
                            .bit(config.mode.polarity == Polarity::IdleHigh)
                         .master()
                            .master()
                         .lsbfrst()
                            .msbfirst()
                         .ssoe()
                            .bit(config.managed_cs == true)
                         .ssm()
                            .bit(config.managed_cs == false)
                         .mssi()
                            .bits(cycle_delay)
                         .ioswp()
                            .bit(config.swap_miso_mosi == true)
                         .comm()
                            .bits(config.communication_mode as u8)
                    });

                    // spe: enable the SPI bus
                    spi.cr1.write(|w| w.ssi().slave_not_selected().spe().enabled());

                    Spi { spi }
                }

                /// Enable interrupts for the given `event`:
                ///  - Received data ready to be read (RXP)
                ///  - Transmit data register empty (TXP)
                ///  - Error
                pub fn listen(&mut self, event: Event) {
                    match event {
                        Event::Rxp => self.spi.ier.modify(|_, w|
                                                          w.rxpie().not_masked()),
                        Event::Txp => self.spi.ier.modify(|_, w|
                                                          w.txpie().not_masked()),
                        Event::Error => self.spi.ier.modify(|_, w| {
                            w.udrie() // Underrun
                                .not_masked()
                                .ovrie() // Overrun
                                .not_masked()
                                .crceie() // CRC error
                                .not_masked()
                                .modfie() // Mode fault
                                .not_masked()
                        }),
                        Event::Eot => self.spi.ier.modify(|_, w| w.eotie().not_masked()),
                    }
                }

                /// Disable interrupts for the given `event`:
                ///  - Received data ready to be read (RXP)
                ///  - Transmit data register empty (TXP)
                ///  - Error
                pub fn unlisten(&mut self, event: Event) {
                    match event {
                        Event::Rxp => self.spi.ier.modify(|_, w|
                                                          w.rxpie().masked()),
                        Event::Txp => self.spi.ier.modify(|_, w|
                                                          w.txpie().masked()),
                        Event::Error => self.spi.ier.modify(|_, w| {
                            w.udrie() // Underrun
                                .masked()
                                .ovrie() // Overrun
                                .masked()
                                .crceie() // CRC error
                                .masked()
                                .modfie() // Mode fault
                                .masked()
                        }),
                        Event::Eot => self.spi.ier.modify(|_, w| w.eotie().masked()),
                    }
                }

                /// Return `true` if the TXP flag is set, i.e. new
                /// data to transmit can be written to the SPI.
                pub fn is_txp(&self) -> bool {
                    self.spi.sr.read().txp().is_not_full()
                }

                /// Return `true` if the RXP flag is set, i.e. new
                /// data has been received and can be read from the
                /// SPI.
                pub fn is_rxp(&self) -> bool {
                    self.spi.sr.read().rxp().is_not_empty()
                }

                /// Return `true` if the MODF flag is set, i.e. the
                /// SPI has experienced a mode fault
                pub fn is_modf(&self) -> bool {
                    self.spi.sr.read().modf().is_fault()
                }

                /// Return `true` if the OVR flag is set, i.e. new
                /// data has been received while the receive data
                /// register was already filled.
                pub fn is_ovr(&self) -> bool {
                    self.spi.sr.read().ovr().is_overrun()
                }

                pub fn free(self) -> $SPIX {
                    self.spi
                }
            }

            impl SpiExt<$SPIX> for $SPIX {
	            fn spi<PINS, T, CONFIG>(self,
                                _pins: PINS,
                                config: CONFIG,
                                freq: T,
                                ccdr: &Ccdr) -> Spi<$SPIX>
	            where
	                PINS: Pins<$SPIX>,
	                T: Into<Hertz>,
                    CONFIG: Into<Config>,
	            {
	                Spi::$spiX(self, config, freq, ccdr)
	            }

	            fn spi_unchecked<T, CONFIG>(self,
                                config: CONFIG,
                                freq: T,
                                ccdr: &Ccdr) -> Spi<$SPIX>
	            where
	                T: Into<Hertz>,
                    CONFIG: Into<Config>,
	            {
	                Spi::$spiX(self, config, freq, ccdr)
	            }
	        }

            impl<T> hal::spi::FullDuplex<T> for Spi<$SPIX> {
                type Error = Error;

                fn read(&mut self) -> nb::Result<T, Error> {
                    let sr = self.spi.sr.read();

                    Err(if sr.ovr().is_overrun() {
                        nb::Error::Other(Error::Overrun)
                    } else if sr.modf().is_fault() {
                        nb::Error::Other(Error::ModeFault)
                    } else if sr.crce().is_error() {
                        nb::Error::Other(Error::Crc)
                    } else if sr.rxp().is_not_empty() {
                        // NOTE(read_volatile) read only 1 byte (the
                        // svd2rust API only allows reading a
                        // half-word)
                        return Ok(unsafe {
                            ptr::read_volatile(
                                &self.spi.rxdr as *const _ as *const T,
                            )
                        });
                    } else {
                        nb::Error::WouldBlock
                    })
                }

                fn send(&mut self, byte: T) -> nb::Result<(), Error> {
                    let sr = self.spi.sr.read();

                    Err(if sr.ovr().is_overrun() {
                        nb::Error::Other(Error::Overrun)
                    } else if sr.modf().is_fault() {
                        nb::Error::Other(Error::ModeFault)
                    } else if sr.crce().is_error() {
                        nb::Error::Other(Error::Crc)
                    } else if sr.txp().is_not_full() {
                        // NOTE(write_volatile) see note above
                        unsafe {
                            ptr::write_volatile(
                                &self.spi.txdr as *const _ as *mut T,
                                byte,
                            )
                        }
                        // write CSTART to start a transaction in
                        // master mode
                        self.spi.cr1.modify(|_, w| w.cstart().started());

                        return Ok(());
                    } else {
                        nb::Error::WouldBlock
                    })
                }
            }

            // For each $TY
            $(
                impl hal::blocking::spi::transfer::Default<$TY>
                    for Spi<$SPIX> {}

                impl hal::blocking::spi::write::Default<$TY>
                    for Spi<$SPIX> {}
            )+
        )+
	}
}

macro_rules! spi123sel {
	($($SPIX:ident,)+) => {
	    $(
            impl Spi<$SPIX> {
                /// Returns the frequency of the current kernel clock
                /// for SPI1, SPI2, SPI3
                fn kernel_clk(ccdr: &Ccdr) -> Option<Hertz> {
                    match ccdr.rb.d2ccip1r.read().spi123sel().variant() {
                        Val(d2ccip1r::SPI123SEL_A::PLL1_Q) => ccdr.clocks.pll1_q_ck(),
                        Val(d2ccip1r::SPI123SEL_A::PLL2_P) => ccdr.clocks.pll2_p_ck(),
                        Val(d2ccip1r::SPI123SEL_A::PLL3_P) => ccdr.clocks.pll3_p_ck(),
                        // Need a method of specifying pin clock
                        Val(d2ccip1r::SPI123SEL_A::I2S_CKIN) => unimplemented!(),
                        Val(d2ccip1r::SPI123SEL_A::PER) => ccdr.clocks.per_ck(),
                        _ => unreachable!(),
                    }
                }
            }
        )+
    }
}
macro_rules! spi45sel {
	($($SPIX:ident,)+) => {
	    $(
            impl Spi<$SPIX> {
                /// Returns the frequency of the current kernel clock
                /// for SPI4, SPI5
                fn kernel_clk(ccdr: &Ccdr) -> Option<Hertz> {
                    match ccdr.rb.d2ccip1r.read().spi45sel().variant() {
                        Val(d2ccip1r::SPI45SEL_A::APB) => Some(ccdr.clocks.pclk2()),
                        Val(d2ccip1r::SPI45SEL_A::PLL2_Q) => ccdr.clocks.pll2_q_ck(),
                        Val(d2ccip1r::SPI45SEL_A::PLL3_Q) => ccdr.clocks.pll3_q_ck(),
                        Val(d2ccip1r::SPI45SEL_A::HSI_KER) => ccdr.clocks.hsi_ck(),
                        Val(d2ccip1r::SPI45SEL_A::CSI_KER) => ccdr.clocks.csi_ck(),
                        Val(d2ccip1r::SPI45SEL_A::HSE) => ccdr.clocks.hse_ck(),
                        _ => unreachable!(),
                    }
                }
            }
        )+
    }
}
macro_rules! spi6sel {
	($($SPIX:ident,)+) => {
	    $(
            impl Spi<$SPIX> {
                /// Returns the frequency of the current kernel clock
                /// for SPI6
                fn kernel_clk(ccdr: &Ccdr) -> Option<Hertz> {
                    match ccdr.rb.d3ccipr.read().spi6sel().variant() {
                        Val(d3ccipr::SPI6SEL_A::RCC_PCLK4) => Some(ccdr.clocks.pclk4()),
                        Val(d3ccipr::SPI6SEL_A::PLL2_Q) => ccdr.clocks.pll2_q_ck(),
                        Val(d3ccipr::SPI6SEL_A::PLL3_Q) => ccdr.clocks.pll3_q_ck(),
                        Val(d3ccipr::SPI6SEL_A::HSI_KER) => ccdr.clocks.hsi_ck(),
                        Val(d3ccipr::SPI6SEL_A::CSI_KER) => ccdr.clocks.csi_ck(),
                        Val(d3ccipr::SPI6SEL_A::HSE) => ccdr.clocks.hse_ck(),
                        _ => unreachable!(),
                    }
                }
            }
        )+
    }
}

spi! {
    SPI1: (spi1, apb2enr,  spi1en, pclk2) => (u8, u16, u32),
    SPI2: (spi2, apb1lenr, spi2en, pclk1) => (u8, u16, u32),
    SPI3: (spi3, apb1lenr, spi3en, pclk1) => (u8, u16, u32),
    SPI4: (spi4, apb2enr,  spi4en, pclk2) => (u8, u16, u32),
    SPI5: (spi5, apb2enr,  spi5en, pclk2) => (u8, u16, u32),
    SPI6: (spi6, apb4enr,  spi6en, pclk2) => (u8, u16, u32),
}

spi123sel! {
    SPI1, SPI2, SPI3,
}
spi45sel! {
    SPI4, SPI5,
}
spi6sel! {
    SPI6,
}
