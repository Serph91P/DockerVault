use crate::timestamp::Timestamp;
use std::collections::{BTreeMap, BTreeSet};

pub struct Policy {
    pub keep_last: i64,
    pub buckets: [i64; 4],
    pub max_age_days: i64,
}

/// Select from completed backups ordered by their stored creation time, newest first.
pub fn keep(rows: &[(i64, Timestamp)], policy: &Policy, now_micros: i64) -> BTreeSet<i64> {
    let cutoff = i128::from(now_micros) - i128::from(policy.max_age_days) * 86_400_000_000;
    let mut kept = BTreeSet::new();
    kept.extend(
        rows.iter()
            .take(usize::try_from(policy.keep_last.max(1)).unwrap_or(usize::MAX))
            .map(|(id, _)| *id),
    );
    let mut periods: [_; 4] = std::array::from_fn(|_| BTreeMap::new());
    for &(id, stamp) in rows {
        let wall = stamp.naive.as_utc().unix_timestamp_nanos() / 1_000;
        if wall - i128::from(stamp.offset.unwrap_or_default()) < cutoff {
            continue;
        }
        let date = stamp.naive.date();
        let year = date.year();
        // Python uses strftime("%Y-W%V"), not the ISO week-year (%G).
        let keys = [
            (year, date.ordinal()),
            (year, u16::from(date.iso_week())),
            (year, u16::from(date.month() as u8)),
            (year, 0),
        ];
        for (period, key) in periods.iter_mut().zip(keys) {
            period.entry(key).or_insert(id);
        }
    }
    for (period, n) in periods.iter().zip(policy.buckets) {
        let count = if n >= 0 {
            usize::try_from(n).unwrap_or(usize::MAX)
        } else {
            period
                .len()
                .saturating_sub(usize::try_from(n.unsigned_abs()).unwrap_or(usize::MAX))
        };
        kept.extend(period.values().rev().take(count).copied());
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(input: &[&str]) -> Vec<(i64, Timestamp)> {
        input
            .iter()
            .enumerate()
            .map(|(i, text)| (i as i64 + 1, Timestamp::parse(text).unwrap()))
            .collect()
    }

    #[test]
    fn keeps_last_independently_of_age_and_falls_back_to_the_newest() {
        let rows = rows(&[
            "2025-01-01T03:00:00",
            "2024-12-31T03:00:00",
            "2023-12-30T03:00:00",
        ]);
        let now = Timestamp::parse("2025-01-03T00:00:00Z")
            .unwrap()
            .unix_micros()
            .unwrap();
        let mut policy = Policy {
            keep_last: 2,
            buckets: [7, 4, 6, 2],
            max_age_days: 1,
        };
        assert_eq!(keep(&rows, &policy, now), BTreeSet::from([1, 2]));
        policy.keep_last = 0;
        assert_eq!(keep(&rows, &policy, now), BTreeSet::from([1]));
        assert!(keep(&[], &policy, now).is_empty());
    }

    #[test]
    fn uses_calendar_year_with_iso_week_and_includes_the_age_boundary() {
        let rows = rows(&[
            "2025-01-02T03:00:00",
            "2025-01-02T02:00:00",
            "2024-12-31T03:00:00",
            "2024-12-29T03:00:00",
            "2024-12-29T02:59:59",
        ]);
        let now = Timestamp::parse("2025-01-02T03:00:00Z")
            .unwrap()
            .unix_micros()
            .unwrap();
        let mut policy = Policy {
            keep_last: 2,
            buckets: [0, 2, 0, 0],
            max_age_days: 4,
        };
        assert_eq!(keep(&rows, &policy, now), BTreeSet::from([1, 2, 4]));
        policy.buckets = [0, 0, 2, 0];
        assert_eq!(keep(&rows, &policy, now), BTreeSet::from([1, 2, 3]));
        policy.buckets = [4, 0, 0, 0];
        assert_eq!(keep(&rows, &policy, now), BTreeSet::from([1, 2, 3, 4]));
    }
}
