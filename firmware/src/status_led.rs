use esp_idf_svc::{
    hal::{
        gpio::OutputPin,
        rmt::{
            PinState, Symbol, TxChannelDriver,
            config::{TransmitConfig, TxChannelConfig},
            encoder::CopyEncoder,
        },
        units::Hertz,
    },
    sys::EspError,
};
use mappin_box_model::indicator::{Pattern, Rgb};
use std::time::{Duration, Instant};

const RESOLUTION: Hertz = Hertz(10_000_000);

pub struct StatusLed {
    channel: TxChannelDriver<'static>,
    encoder: CopyEncoder,
    zero: Symbol,
    one: Symbol,
    latch: Symbol,
    pattern: Option<Pattern>,
    since: Instant,
    color: Option<Rgb>,
}

impl StatusLed {
    pub fn new(pin: impl OutputPin + 'static) -> Result<Self, EspError> {
        let bit = |high, low| {
            Symbol::new_with(
                RESOLUTION,
                PinState::High,
                Duration::from_nanos(high),
                PinState::Low,
                Duration::from_nanos(low),
            )
        };
        Ok(Self {
            channel: TxChannelDriver::new(
                pin,
                &TxChannelConfig {
                    resolution: RESOLUTION,
                    transaction_queue_depth: 1,
                    ..Default::default()
                },
            )?,
            encoder: CopyEncoder::new()?,
            zero: bit(300, 900)?,
            one: bit(900, 300)?,
            latch: Symbol::new_half_split(
                RESOLUTION,
                PinState::Low,
                PinState::Low,
                Duration::from_micros(320),
            )?,
            pattern: None,
            since: Instant::now(),
            color: None,
        })
    }

    fn draw(
        &mut self,
        pattern: Pattern,
        notice_at: Option<Instant>,
        now: Instant,
    ) -> Result<(), EspError> {
        let changed = self.pattern != Some(pattern);
        if changed {
            self.since = now;
        }
        let elapsed = now.saturating_duration_since(notice_at.unwrap_or(self.since));
        if let Some(color) = pattern.color(elapsed)
            && self.color != Some(color)
        {
            let [red, green, blue] = color;
            // The board's WS2812 receives one GRB pixel followed by a low latch.
            let pixel = u32::from_be_bytes([0, green, red, blue]);
            let mut frame = [self.latch; 25];
            for (i, symbol) in frame[..24].iter_mut().enumerate() {
                *symbol = if pixel & (1 << (23 - i)) == 0 {
                    self.zero
                } else {
                    self.one
                };
            }
            self.channel
                .send_and_wait(&mut self.encoder, &frame, &TransmitConfig::default())?;
            self.color = Some(color);
        }
        if changed {
            self.pattern = Some(pattern);
            log::info!("MAPPIN_LED_STATE: {pattern:?}");
        }
        Ok(())
    }

    pub fn show(
        led: &mut Option<Self>,
        pattern: Pattern,
        notice_at: Option<Instant>,
        now: Instant,
    ) {
        if let Some(driver) = led
            && let Err(error) = driver.draw(pattern, notice_at, now)
        {
            log::warn!("MAPPIN_LED_DISABLED: {error}");
            *led = None;
        }
    }
}
