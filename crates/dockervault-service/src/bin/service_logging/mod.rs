use dockervault_service::config::Config;
use std::fmt;
use tracing::{
    Event, Level, Subscriber,
    field::{Field, Visit},
    level_filters::LevelFilter,
};
use tracing_subscriber::{
    filter::Targets,
    fmt::{
        FmtContext, FormatEvent, FormatFields,
        format::Writer,
        time::{FormatTime, SystemTime},
    },
    layer::SubscriberExt,
    registry::LookupSpan,
    util::SubscriberInitExt,
};

pub(super) fn configure(config: &Config) -> Result<(), &'static str> {
    let level = match config.log_level.to_ascii_uppercase().as_str() {
        "DEBUG" => LevelFilter::DEBUG,
        "WARN" | "WARNING" => LevelFilter::WARN,
        "ERROR" => LevelFilter::ERROR,
        "CRITICAL" | "FATAL" => LevelFilter::OFF,
        _ => LevelFilter::INFO,
    };
    let formatter = ServiceFormat {
        json: config.log_format.eq_ignore_ascii_case("json"),
    };
    tracing_subscriber::registry()
        // Protocol library tracing may contain request or credential details.
        .with(Targets::new().with_target("dockervault_service", level))
        .with(
            tracing_subscriber::fmt::layer()
                .event_format(formatter)
                .with_ansi(false),
        )
        .try_init()
        .map_err(|_| "log initialization")
}

struct ServiceFormat {
    json: bool,
}

#[derive(Default)]
struct Message(String);

impl Visit for Message {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{value:?}");
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.0 = value.into();
        }
    }
}

impl<S, N> FormatEvent<S, N> for ServiceFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        _: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let metadata = event.metadata();
        let level = if *metadata.level() == Level::WARN {
            "WARNING"
        } else {
            metadata.level().as_str()
        };
        let logger = metadata.target();
        let mut timestamp = String::new();
        SystemTime.format_time(&mut Writer::new(&mut timestamp))?;
        // Keep messages, not arbitrary provider, subprocess or SQL error fields.
        let mut message = Message::default();
        event.record(&mut message);
        if self.json {
            writeln!(
                writer,
                "{}",
                serde_json::json!({"timestamp":timestamp,"level":level,"logger":logger,"message":message.0})
            )
        } else {
            writeln!(writer, "{timestamp} - {logger} - {level} - {}", message.0)
        }
    }
}
