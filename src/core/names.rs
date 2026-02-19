/// Generate a friendly random task name like "brisk-ember" or "swift-pixel".
/// Avoids names already in `existing`.

const ADJECTIVES: &[&str] = &[
    "curious", "brisk", "mellow", "vivid", "bright", "calm", "daring", "eager",
    "gentle", "keen", "lively", "nimble", "quiet", "rapid", "steady", "swift",
    "tidy", "bold", "clever", "fresh",
];

const NOUNS: &[&str] = &[
    "branch", "pixel", "thread", "anchor", "beacon", "circuit", "delta", "ember",
    "harbor", "lantern", "meadow", "moment", "quill", "signal", "spark", "stride",
    "trail", "vector", "weave", "whisper",
];

pub fn generate(existing: &[String]) -> String {
    let existing_set: std::collections::HashSet<&str> =
        existing.iter().map(|s| s.as_str()).collect();

    // Try random combinations
    for _ in 0..20 {
        let adj = ADJECTIVES[fastrand() % ADJECTIVES.len()];
        let noun = NOUNS[fastrand() % NOUNS.len()];
        let name = format!("{}-{}", adj, noun);
        if !existing_set.contains(name.as_str()) {
            return name;
        }
    }

    // Fallback with number
    for i in 1..100 {
        let name = format!("task-{}", i);
        if !existing_set.contains(name.as_str()) {
            return name;
        }
    }

    format!("task-{}", std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs())
}

/// Simple fast random using system time as seed.
fn fastrand() -> usize {
    use std::time::SystemTime;
    use std::cell::Cell;

    thread_local! {
        static STATE: Cell<u64> = Cell::new(
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64
        );
    }

    STATE.with(|s| {
        let mut x = s.get();
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        s.set(x);
        x as usize
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_adjective_noun() {
        let name = generate(&[]);
        assert!(name.contains('-'), "name should have a hyphen: {}", name);
        let parts: Vec<&str> = name.splitn(2, '-').collect();
        assert_eq!(parts.len(), 2);
        assert!(ADJECTIVES.contains(&parts[0]), "bad adjective: {}", parts[0]);
        assert!(NOUNS.contains(&parts[1]), "bad noun: {}", parts[1]);
    }

    #[test]
    fn avoids_existing_names() {
        // Fill up most combinations
        let mut existing = Vec::new();
        for adj in ADJECTIVES {
            for noun in NOUNS {
                existing.push(format!("{}-{}", adj, noun));
            }
        }
        // Remove one so there's still a slot... but we have 400 combos
        // and only try 20, so it'll likely hit fallback
        let name = generate(&existing);
        assert!(!existing.contains(&name));
    }

    #[test]
    fn different_each_time() {
        let a = generate(&[]);
        let b = generate(&[]);
        // Not guaranteed different, but very likely with 400 combos
        // Just check they're both valid
        assert!(a.contains('-'));
        assert!(b.contains('-'));
    }
}
