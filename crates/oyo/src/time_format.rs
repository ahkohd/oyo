use crate::config::{TimeConfig, TimeMode};
use time::format_description::{parse_owned, parse_strftime_owned, OwnedFormatItem};
use time::OffsetDateTime;

const DEFAULT_ABSOLUTE_FORMAT: &str = "[year]-[month]-[day] [hour]:[minute]";

#[derive(Debug, Clone)]
pub struct TimeFormatter {
    mode: TimeMode,
    absolute_format: OwnedFormatItem,
    custom_format: Option<OwnedFormatItem>,
}

impl Default for TimeFormatter {
    fn default() -> Self {
        Self::new(&TimeConfig::default())
    }
}

impl TimeFormatter {
    pub fn new(config: &TimeConfig) -> Self {
        let absolute_format = parse_owned::<2>(DEFAULT_ABSOLUTE_FORMAT)
            .or_else(|_| parse_strftime_owned("%Y-%m-%d %H:%M"))
            .expect("default time format should parse");
        let custom_format = match config.mode {
            TimeMode::Custom => parse_format(&config.format),
            _ => None,
        };
        Self {
            mode: config.mode,
            absolute_format,
            custom_format,
        }
    }

    pub fn format(&self, epoch: Option<i64>, now: i64) -> String {
        let Some(epoch) = epoch else {
            return "Unknown".to_string();
        };
        match self.mode {
            TimeMode::Relative => format_relative_age(epoch, now),
            TimeMode::Absolute => format_absolute(epoch, &self.absolute_format)
                .unwrap_or_else(|| "Unknown".to_string()),
            TimeMode::Custom => {
                let format = self.custom_format.as_ref().unwrap_or(&self.absolute_format);
                format_absolute(epoch, format).unwrap_or_else(|| "Unknown".to_string())
            }
        }
    }
}

fn parse_format(format: &str) -> Option<OwnedFormatItem> {
    let trimmed = format.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.contains('%') {
        parse_strftime_owned(trimmed).ok()
    } else {
        parse_owned::<2>(trimmed).ok()
    }
}

fn format_absolute(epoch: i64, format: &OwnedFormatItem) -> Option<String> {
    let date_time = OffsetDateTime::from_unix_timestamp(epoch).ok()?;
    date_time.format(format).ok()
}

pub fn format_relative_age(epoch: i64, now: i64) -> String {
    let age_secs = now.saturating_sub(epoch);
    let age_days = age_secs / 86_400;
    if age_days <= 0 {
        return "today".to_string();
    }
    if age_days == 1 {
        return "1 day ago".to_string();
    }
    if age_days < 30 {
        return format!("{age_days} days ago");
    }
    if age_days < 365 {
        let months = (age_days / 30).max(1);
        if months == 1 {
            return "1 month ago".to_string();
        }
        return format!("{months} months ago");
    }
    let years = age_days / 365;
    if years == 1 {
        "1 year ago".to_string()
    } else {
        format!("{years} years ago")
    }
}
