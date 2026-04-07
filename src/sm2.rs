/// SM-2 spaced repetition algorithm.
///
/// quality: 0–5
///   0–2 = failed (blackout / incorrect)
///   3   = correct but hard
///   4   = correct with some hesitation
///   5   = perfect recall
pub struct Sm2Result {
    pub repetitions: i64,
    pub ease_factor: f64,
    pub interval_days: i64,
    /// 0.0–1.0; reaches 1.0 after 5 consecutive successful reviews
    pub mastery: f64,
}

pub fn update(quality: u8, repetitions: i64, ease_factor: f64, interval_days: i64) -> Sm2Result {
    let q = quality.min(5) as f64;

    let (new_reps, new_interval, new_ef) = if q < 3.0 {
        // Failed: reset streak, keep ease_factor unchanged
        (0i64, 1i64, ease_factor)
    } else {
        let new_interval = match repetitions {
            0 => 1,
            1 => 6,
            _ => (interval_days as f64 * ease_factor).round() as i64,
        };
        let new_ef = (ease_factor + 0.1 - (5.0 - q) * (0.08 + (5.0 - q) * 0.02)).max(1.3);
        (repetitions + 1, new_interval, new_ef)
    };

    // Mastery 0.0→1.0: full mastery after 5 consecutive successful reviews
    let mastery = (new_reps as f64 / 5.0).clamp(0.0, 1.0);

    Sm2Result {
        repetitions: new_reps,
        ease_factor: new_ef,
        interval_days: new_interval,
        mastery,
    }
}
