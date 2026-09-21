//! iOS diagnostics keep application detail without the transport firehose.
//! Used only on iOS (and in host-side unit tests); desktop logging is unchanged.
pub fn transport_level(_: log::LevelFilter) -> log::LevelFilter {
    log::LevelFilter::Warn
}

#[cfg(test)]
mod tests {
    #[test]
    fn ios_transport_never_inherits_verbose_diagnostics() {
        for requested in [log::LevelFilter::Off, log::LevelFilter::Warn,
            log::LevelFilter::Info, log::LevelFilter::Debug, log::LevelFilter::Trace] {
            assert_eq!(super::transport_level(requested), log::LevelFilter::Warn);
        }
    }
}
