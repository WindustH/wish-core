//! SQLite-compatible calendar queries used by usage reports.
use super::repository_error::RepositoryError;
/// Dates read and written the way SQLite reads and writes them.
pub struct Calendar(rusqlite::Connection);
impl Calendar {
  pub fn open() -> Result<Self, RepositoryError> {
    Ok(Self(rusqlite::Connection::open_in_memory()?))
  }
  /// Unix milliseconds at the start of a UTC date, or none when it is not one.
  pub fn parse_day_start(&self, date: &str) -> Result<Option<i64>, RepositoryError> {
    Ok(self.0.query_row("SELECT unixepoch(?1)*1000", [date], |r| r.get(0))?)
  }
  /// The UTC date at unix milliseconds.
  pub fn format_date(&self, ms: i64) -> Result<String, RepositoryError> {
    Ok(self.0.query_row("SELECT date(?1/1000,'unixepoch')", [ms], |r| r.get(0))?)
  }
}
