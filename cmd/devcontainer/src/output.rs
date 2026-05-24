//! Output rendering helpers for JSON and human-readable command results.

use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy)]
pub enum LogFormat {
    Text,
    Json,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandLogLevel {
    Trace,
    Debug,
    Info,
    Error,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TerminalDimensions {
    pub columns: usize,
    pub rows: usize,
}

pub struct CommandLogger {
    format: LogFormat,
    level: CommandLogLevel,
    terminal_dimensions: Option<TerminalDimensions>,
}

impl CommandLogLevel {
    fn severity(self) -> u8 {
        match self {
            Self::Trace => 0,
            Self::Debug => 1,
            Self::Info => 2,
            Self::Error => 3,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Trace => "trace",
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Error => "error",
        }
    }

    fn upstream_level(self) -> u8 {
        match self {
            Self::Trace => 1,
            Self::Debug => 2,
            Self::Info => 3,
            Self::Error => 5,
        }
    }
}

impl CommandLogger {
    pub fn new(format: LogFormat, level: CommandLogLevel) -> Self {
        Self {
            format,
            level,
            terminal_dimensions: None,
        }
    }

    pub fn with_terminal_dimensions(
        mut self,
        terminal_dimensions: Option<TerminalDimensions>,
    ) -> Self {
        self.terminal_dimensions = terminal_dimensions;
        self
    }

    pub fn error(&self, message: impl AsRef<str>) {
        self.log(CommandLogLevel::Error, message);
    }

    pub fn info(&self, message: impl AsRef<str>) {
        self.log(CommandLogLevel::Info, message);
    }

    pub fn debug(&self, message: impl AsRef<str>) {
        self.log(CommandLogLevel::Debug, message);
    }

    pub fn trace(&self, message: impl AsRef<str>) {
        self.log(CommandLogLevel::Trace, message);
    }

    pub fn trace_terminal_dimensions(&self) {
        if let Some(dimensions) = self.terminal_dimensions {
            self.trace(format!(
                "Using terminal dimensions: columns={} rows={}",
                dimensions.columns, dimensions.rows
            ));
        }
    }

    fn log(&self, level: CommandLogLevel, message: impl AsRef<str>) {
        if let Some(rendered) = self.render(level, message.as_ref()) {
            eprintln!("{rendered}");
        }
    }

    fn render(&self, level: CommandLogLevel, message: &str) -> Option<String> {
        if level.severity() < self.level.severity() {
            return None;
        }

        Some(match self.format {
            LogFormat::Text => format!("[{}] {message}", level.as_str()),
            LogFormat::Json => render_log(self.format, level, message),
        })
    }
}

pub fn render_log(format: LogFormat, level: CommandLogLevel, message: &str) -> String {
    match format {
        LogFormat::Text => message.to_string(),
        LogFormat::Json => serde_json::json!({
            "type": "text",
            "level": level.upstream_level(),
            "timestamp": log_timestamp(),
            "text": message,
        })
        .to_string(),
    }
}

fn log_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::{render_log, CommandLogLevel, CommandLogger, LogFormat, TerminalDimensions};

    #[test]
    fn renders_text_logs_without_json_envelope() {
        assert_eq!(
            render_log(LogFormat::Text, CommandLogLevel::Info, "hello"),
            "hello"
        );
    }

    #[test]
    fn renders_json_logs_as_upstream_text_events() {
        let rendered: Value = serde_json::from_str(&render_log(
            LogFormat::Json,
            CommandLogLevel::Info,
            "quoted \"value\"",
        ))
        .expect("json log");

        assert_eq!(rendered["type"], "text");
        assert_eq!(rendered["level"], 3);
        assert_eq!(rendered["text"], "quoted \"value\"");
        assert!(rendered["timestamp"].as_i64().is_some(), "{rendered:?}");
    }

    #[test]
    fn command_logger_filters_entries_below_configured_level() {
        let logger = CommandLogger::new(LogFormat::Text, CommandLogLevel::Info);

        assert_eq!(logger.render(CommandLogLevel::Trace, "ignored"), None);
        assert_eq!(
            logger.render(CommandLogLevel::Error, "emitted"),
            Some("[error] emitted".to_string())
        );
    }

    #[test]
    fn command_logger_renders_all_text_levels() {
        let logger = CommandLogger::new(LogFormat::Text, CommandLogLevel::Trace);

        assert_eq!(
            logger.render(CommandLogLevel::Trace, "traced"),
            Some("[trace] traced".to_string())
        );
        assert_eq!(
            logger.render(CommandLogLevel::Debug, "debugged"),
            Some("[debug] debugged".to_string())
        );
        assert_eq!(
            logger.render(CommandLogLevel::Info, "informed"),
            Some("[info] informed".to_string())
        );
        assert_eq!(
            logger.render(CommandLogLevel::Error, "errored"),
            Some("[error] errored".to_string())
        );
    }

    #[test]
    fn command_logger_renders_json_entries() {
        let logger = CommandLogger::new(LogFormat::Json, CommandLogLevel::Trace);

        let rendered = logger
            .render(CommandLogLevel::Debug, "quoted \"value\"")
            .expect("json log");
        let entry: Value = serde_json::from_str(&rendered).expect("json log");

        assert_eq!(entry["type"], "text");
        assert_eq!(entry["level"], 2);
        assert_eq!(entry["text"], "quoted \"value\"");
        assert!(entry["timestamp"].as_i64().is_some(), "{entry:?}");
    }

    #[test]
    fn command_logger_renders_json_trace_and_error_levels() {
        let logger = CommandLogger::new(LogFormat::Json, CommandLogLevel::Trace);

        let trace: Value = serde_json::from_str(
            &logger
                .render(CommandLogLevel::Trace, "trace")
                .expect("trace log"),
        )
        .expect("trace json");
        let error: Value = serde_json::from_str(
            &logger
                .render(CommandLogLevel::Error, "error")
                .expect("error log"),
        )
        .expect("error json");

        assert_eq!(trace["level"], 1);
        assert_eq!(error["level"], 5);
    }

    #[test]
    fn command_logger_stores_terminal_dimensions() {
        let logger = CommandLogger::new(LogFormat::Text, CommandLogLevel::Trace)
            .with_terminal_dimensions(Some(TerminalDimensions {
                columns: 120,
                rows: 40,
            }));

        assert_eq!(
            logger.terminal_dimensions,
            Some(TerminalDimensions {
                columns: 120,
                rows: 40,
            })
        );
    }

    #[test]
    fn command_logger_public_methods_emit_without_panicking() {
        let logger = CommandLogger::new(LogFormat::Text, CommandLogLevel::Trace)
            .with_terminal_dimensions(Some(TerminalDimensions {
                columns: 120,
                rows: 40,
            }));

        logger.error("error");
        logger.info("info");
        logger.debug("debug");
        logger.trace("trace");
        logger.trace_terminal_dimensions();
    }
}
