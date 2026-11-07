//! Summary statistics for the console report.

/// Median, quartiles and extremes of a sample.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Summary {
    pub n: usize,
    pub min: f64,
    pub q1: f64,
    pub median: f64,
    pub q3: f64,
    pub max: f64,
}

/// Linear-interpolated quantile of sorted data (the "type 7" definition
/// numpy and R use by default, so `analyze.py` agrees with it).
fn quantile(sorted: &[f64], q: f64) -> f64 {
    let pos = q * (sorted.len() - 1) as f64;
    let lo = pos.floor() as usize;
    let hi = pos.ceil() as usize;
    sorted[lo] + (sorted[hi] - sorted[lo]) * (pos - lo as f64)
}

/// Summarise `values`; `None` when empty.
pub fn summarize(values: &[f64]) -> Option<Summary> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    Some(Summary {
        n: sorted.len(),
        min: sorted[0],
        q1: quantile(&sorted, 0.25),
        median: quantile(&sorted, 0.5),
        q3: quantile(&sorted, 0.75),
        max: sorted[sorted.len() - 1],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn odd_and_even_medians() {
        assert_eq!(summarize(&[3.0, 1.0, 2.0]).unwrap().median, 2.0);
        assert_eq!(summarize(&[4.0, 1.0, 3.0, 2.0]).unwrap().median, 2.5);
    }

    #[test]
    fn quartiles_match_numpy_default() {
        // numpy.percentile([1..=8], [25, 75]) == [2.75, 6.25]
        let s = summarize(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0]).unwrap();
        assert_eq!((s.q1, s.q3), (2.75, 6.25));
        assert_eq!((s.min, s.max, s.n), (1.0, 8.0, 8));
    }

    #[test]
    fn empty_has_no_summary() {
        assert_eq!(summarize(&[]), None);
    }
}
