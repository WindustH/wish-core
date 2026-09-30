//! Wall-clock time, read and spelled the few ways this crate needs.

/// Unix time in milliseconds. Ordering is defined by history sequence numbers, not wall time.
#[derive(
  Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct Timestamp(pub u64);
impl Timestamp {
  pub fn now() -> Self {
    Self(
      std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64,
    )
  }
}

/// Seconds since the Unix epoch by the system clock, or `0` from a clock set before 1970.
pub fn unix_seconds() -> u64 {
  std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .map_or(0, |elapsed| elapsed.as_secs())
}

/// The UTC date `(year, month, day)` a count of days since 1970-01-01 falls on.
///
/// Howard Hinnant's civil-from-days, the one calendar conversion a timestamp needs: the days are
/// counted in 400-year eras from 0000-03-01, so a leap day is the last day of its year and
/// needs no special case.
pub fn civil_date(days: u64) -> (i64, i64, i64) {
  let days = i64::try_from(days).unwrap_or(0) + 719_468;
  let era = days.div_euclid(146_097);
  let day_of_era = days.rem_euclid(146_097);
  let year_of_era =
    (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
  let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
  let month_index = (5 * day_of_year + 2) / 153;
  let day = day_of_year - (153 * month_index + 2) / 5 + 1;
  let month = if month_index < 10 { month_index + 3 } else { month_index - 9 };
  (year_of_era + era * 400 + i64::from(month <= 2), month, day)
}
