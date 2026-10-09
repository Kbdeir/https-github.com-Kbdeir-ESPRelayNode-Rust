use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Schedule {
    pub enabled: bool,
    pub relay: Option<u8>,
    pub kind: u8,
    pub date_from: String,
    pub date_to: String,
    pub time_from: String,
    pub time_to: String,
    pub duration_minutes: u32,
    pub weekdays: [bool; 7],
    pub month_day: u8,
}
impl Default for Schedule {
    fn default() -> Self {
        Self {
            enabled: false,
            relay: Some(0),
            kind: 2,
            date_from: "2026-01-01".into(),
            date_to: "2100-12-31".into(),
            time_from: "00:00".into(),
            time_to: "00:01".into(),
            duration_minutes: 0,
            weekdays: [false; 7],
            month_day: 1,
        }
    }
}
impl Schedule {
    pub fn validate(&self) -> Result<(), String> {
        self.compile().map(|_| ())
    }
    pub fn compile(&self) -> Result<CompiledSchedule, String> {
        let from = NaiveDate::parse_from_str(&self.date_from, "%Y-%m-%d")
            .map_err(|_| "invalid start date")?;
        let to =
            NaiveDate::parse_from_str(&self.date_to, "%Y-%m-%d").map_err(|_| "invalid end date")?;
        let start = NaiveTime::parse_from_str(&self.time_from, "%H:%M")
            .map_err(|_| "invalid start time")?;
        let end =
            NaiveTime::parse_from_str(&self.time_to, "%H:%M").map_err(|_| "invalid end time")?;
        if from > to
            || !(1..=4).contains(&self.kind)
            || self.relay.is_some_and(|r| r != 0)
            || !(1..=31).contains(&self.month_day)
            || self.duration_minutes > 10080
            || (self.kind != 1 && self.duration_minutes > 1440)
        {
            return Err("invalid schedule range, relay, type or duration".into());
        }
        if self.enabled
            && self.duration_minutes == 0
            && (self.kind != 1 || from == to)
            && start == end
        {
            return Err("enabled timer needs a nonempty time window".into());
        }
        if self.enabled && self.kind == 3 && !self.weekdays.iter().any(|v| *v) {
            return Err("weekly timer requires a weekday".into());
        }
        let c = CompiledSchedule {
            config: self.clone(),
            from,
            to,
            start,
            end,
        };
        if self.enabled && self.kind == 1 && c.finish(from) <= from.and_time(start) {
            return Err("specific timer end must follow start".into());
        }
        Ok(c)
    }
}
pub struct CompiledSchedule {
    config: Schedule,
    from: NaiveDate,
    to: NaiveDate,
    start: NaiveTime,
    end: NaiveTime,
}
pub fn local_now(offset_hours: i8) -> Option<NaiveDateTime> {
    let epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    if epoch < 1_704_067_200 {
        return None;
    }
    chrono::DateTime::from_timestamp(epoch as i64 + i64::from(offset_hours) * 3600, 0)
        .map(|v| v.naive_utc())
}
impl CompiledSchedule {
    pub fn countdown(&self, now: NaiveDateTime) -> (i64, i64) {
        if !self.config.enabled {
            return (-1, -1);
        }
        if self.active(now) {
            let started = if self.config.kind == 1 {
                self.from
            } else if now.time() < self.start {
                now.date() - Duration::days(1)
            } else {
                now.date()
            };
            return (-1, (self.finish(started) - now).num_seconds().max(0));
        }
        if self.config.kind == 1 {
            return (
                if now < self.from.and_time(self.start) {
                    (self.from.and_time(self.start) - now).num_seconds()
                } else {
                    -1
                },
                -1,
            );
        }
        let first = if self.config.kind == 3 {
            now.date()
        } else {
            now.date().max(self.from)
        };
        for days in 0..=366 {
            let start = (first + Duration::days(days)).and_time(self.start);
            if start > now && self.active(start) {
                return ((start - now).num_seconds(), -1);
            }
        }
        (-1, -1)
    }
    fn finish(&self, date: NaiveDate) -> NaiveDateTime {
        if self.config.duration_minutes > 0 {
            return date.and_time(self.start)
                + Duration::minutes(i64::from(self.config.duration_minutes));
        }
        if self.config.kind == 1 {
            return self.to.and_time(self.end);
        }
        let date = if self.end < self.start {
            date + Duration::days(1)
        } else {
            date
        };
        date.and_time(self.end)
    }
    pub fn active(&self, now: NaiveDateTime) -> bool {
        if !self.config.enabled {
            return false;
        }
        if self.config.kind == 1 {
            return now >= self.from.and_time(self.start) && now < self.finish(self.from);
        }
        // Anchor overnight windows to their start day, including weekday/month filters.
        [now.date(), now.date() - Duration::days(1)]
            .iter()
            .any(|date| {
                let in_dates = self.config.kind == 3 || (*date >= self.from && *date <= self.to);
                let matches = match self.config.kind {
                    3 => self.config.weekdays[date.weekday().num_days_from_sunday() as usize],
                    4 => date.day() == u32::from(self.config.month_day),
                    _ => true,
                };
                in_dates && matches && now >= date.and_time(self.start) && now < self.finish(*date)
            })
    }
    pub fn relay(&self) -> Option<u8> {
        self.config.relay
    }
}
