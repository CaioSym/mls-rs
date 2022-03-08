use std::time::Duration;

#[cfg(not(target_arch = "wasm32"))]
pub type SystemTimeError = std::time::SystemTimeError;

#[cfg(target_arch = "wasm32")]
pub type SystemTimeError = std::convert::Infallible;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MlsTime(std::time::SystemTime);

#[cfg(not(target_arch = "wasm32"))]
impl MlsTime {
    pub fn now() -> Self {
        Self(std::time::SystemTime::now())
    }

    pub fn from_duration_since_epoch(duration: Duration) -> Option<MlsTime> {
        std::time::SystemTime::UNIX_EPOCH
            .checked_add(duration)
            .map(MlsTime)
    }

    pub fn seconds_since_epoch(&self) -> Result<u64, std::time::SystemTimeError> {
        Ok(self.0.duration_since(std::time::UNIX_EPOCH)?.as_secs())
    }
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(inline_js = r#"
export function date_now() {
  return Date.now();
}"#)]
extern "C" {
    fn date_now() -> f64;
}

#[cfg(target_arch = "wasm32")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MlsTime(u64);

#[cfg(target_arch = "wasm32")]
impl MlsTime {
    pub fn now() -> Self {
        Self((date_now() * 1000.0) as u64)
    }

    pub fn from_duration_since_epoch(duration: Duration) -> Option<MlsTime> {
        Some(MlsTime(duration.as_secs()))
    }

    pub fn seconds_since_epoch(&self) -> Result<u64, SystemTimeError> {
        Ok(Duration::from_micros(self.0).as_secs())
    }
}
