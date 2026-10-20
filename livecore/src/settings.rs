//! The component host's settings, from its program arguments.

use alloc::format;
use alloc::string::{String, ToString};
use kabi::live::valid_label;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// The version to start when the ledger has none.
    pub version: String,
    /// The version started instead when that one cannot be fetched, staged or
    /// started.
    pub fallback: Option<String>,
    /// Requests a candidate must answer before it can be committed.
    pub probation_calls: u64,
    /// Time a candidate must spend on probation before it can be committed. 0 means
    /// calls alone end it, which makes runs repeatable whatever the machine's speed.
    pub probation_ms: u64,
    /// How often a candidate's health is checked while no requests come.
    pub health_ms: u64,
    /// Fuel for one request.
    pub fuel_call: u64,
    /// Fuel for every other call.
    pub fuel_life: u64,
    /// Linear memory per instance.
    pub memory_kib: u64,
}

impl Settings {
    pub const DEFAULT: Settings = Settings {
        version: String::new(),
        fallback: None,
        probation_calls: 16,
        probation_ms: 30_000,
        health_ms: 1000,
        fuel_call: 5_000_000,
        fuel_life: 20_000_000,
        memory_kib: 2048,
    };

    /// Reads `key=value` words. `version` is required; `restart`, which the
    /// supervisor adds, is ignored; anything else unknown is refused.
    pub fn parse(args: &str) -> Result<Self, String> {
        let mut settings = Self::DEFAULT;
        for word in args.split_whitespace() {
            let (key, value) = word
                .split_once('=')
                .ok_or_else(|| format!("expected key=value, got {word:?}"))?;
            let number = |min: u64, max: u64| {
                value
                    .parse::<u64>()
                    .ok()
                    .filter(|n| (min..=max).contains(n))
                    .ok_or_else(|| format!("{key} must be {min} to {max}, got {value:?}"))
            };
            match key {
                "version" if valid_label(value) => settings.version = value.to_string(),
                "version" => return Err(format!("bad version label {value:?}")),
                "fallback" if valid_label(value) => settings.fallback = Some(value.to_string()),
                "fallback" => return Err(format!("bad fallback label {value:?}")),
                "probation_calls" => settings.probation_calls = number(0, 1000)?,
                "probation_ms" => settings.probation_ms = number(0, 600_000)?,
                "health_ms" => settings.health_ms = number(10, 60_000)?,
                "fuel_call" => settings.fuel_call = number(1, 10_000_000_000)?,
                "fuel_life" => settings.fuel_life = number(1, 10_000_000_000)?,
                "memory_kib" => settings.memory_kib = number(64, 16 * 1024)?,
                "restart" => {}
                other => return Err(format!("unknown setting {other:?}")),
            }
        }
        if settings.version.is_empty() {
            return Err("no version= setting".into());
        }
        Ok(settings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plan_arguments_parse_with_defaults_for_the_rest() {
        let settings =
            Settings::parse("restart=2 version=1 probation_calls=3 probation_ms=0").unwrap();
        assert_eq!(settings.version, "1");
        assert_eq!((settings.probation_calls, settings.probation_ms), (3, 0));
        assert_eq!(settings.fuel_call, Settings::DEFAULT.fuel_call);
        assert_eq!(Settings::parse("version=2").unwrap().probation_calls, 16);
        let settings = Settings::parse("version=c-0123456789ab fallback=2").unwrap();
        assert_eq!(settings.fallback.as_deref(), Some("2"));
        assert_eq!(Settings::parse("version=2").unwrap().fallback, None);
    }

    #[test]
    fn missing_bad_or_unknown_settings_are_refused() {
        for bad in [
            "",
            "probation_calls=3",
            "version=Two",
            "version=1 memory_kib=1",
            "version=1 fuel_call=0",
            "version=1 colour=blue",
            "version=1 loose",
            "version=1 fallback=../2",
        ] {
            assert!(Settings::parse(bad).is_err(), "{bad}");
        }
    }
}
