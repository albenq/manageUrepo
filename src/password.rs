//! Configurable strong password generation.
//!
//! Pure domain module: no UI or storage dependencies. Randomness comes from
//! [`rand::rngs::OsRng`] (the operating system CSPRNG).

use rand::rngs::OsRng;
use rand::seq::SliceRandom;
use rand::Rng;

const UPPER: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const LOWER: &str = "abcdefghijklmnopqrstuvwxyz";
const DIGITS: &str = "0123456789";
const SPECIAL: &str = "!@#$%^&*()-_=+[]{};:,.<>?";

/// Which character classes to draw from and how long the result should be.
pub struct GenOptions {
    pub length: usize,
    pub upper: bool,
    pub lower: bool,
    pub digits: bool,
    pub special: bool,
}

impl Default for GenOptions {
    fn default() -> Self {
        Self {
            length: 20,
            upper: true,
            lower: true,
            digits: true,
            special: false,
        }
    }
}

/// Generate a password satisfying `opts`.
///
/// Guarantees at least one character from every enabled class, then fills the
/// remaining positions from the union of enabled classes and shuffles the
/// whole result (Fisher-Yates) so the guaranteed characters are not in fixed
/// positions.
pub fn generate(opts: &GenOptions) -> Result<String, String> {
    let classes: Vec<&str> = [
        (opts.upper, UPPER),
        (opts.lower, LOWER),
        (opts.digits, DIGITS),
        (opts.special, SPECIAL),
    ]
    .iter()
    .filter(|(enabled, _)| *enabled)
    .map(|(_, set)| *set)
    .collect();

    if classes.is_empty() {
        return Err("at least one character class must be enabled".to_string());
    }
    if opts.length == 0 {
        return Err("password length must be greater than zero".to_string());
    }
    if opts.length < classes.len() {
        return Err(format!(
            "password length {} is less than the {} enabled character classes",
            opts.length,
            classes.len()
        ));
    }

    let pool: Vec<char> = classes.iter().flat_map(|set| set.chars()).collect();
    let mut rng = OsRng;

    // One character from every enabled class...
    let mut chars: Vec<char> = Vec::with_capacity(opts.length);
    for set in &classes {
        let set_chars: Vec<char> = set.chars().collect();
        let idx = rng.gen_range(0..set_chars.len());
        chars.push(set_chars[idx]);
    }

    // ...the rest from the union of enabled classes...
    while chars.len() < opts.length {
        let idx = rng.gen_range(0..pool.len());
        chars.push(pool[idx]);
    }

    // ...then shuffle so class positions are not fixed (Fisher-Yates).
    chars.shuffle(&mut rng);

    Ok(chars.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn union(opts: &GenOptions) -> String {
        let mut s = String::new();
        if opts.upper {
            s.push_str(UPPER);
        }
        if opts.lower {
            s.push_str(LOWER);
        }
        if opts.digits {
            s.push_str(DIGITS);
        }
        if opts.special {
            s.push_str(SPECIAL);
        }
        s
    }

    #[test]
    fn default_charset_and_length() {
        let opts = GenOptions::default();
        let pw = generate(&opts).expect("default options must generate");
        assert_eq!(pw.chars().count(), 20);
        assert!(pw.chars().all(|c| c.is_ascii_alphanumeric()));
        assert!(!pw.chars().any(|c| SPECIAL.contains(c)));
    }

    #[test]
    fn special_included_when_enabled() {
        let opts = GenOptions {
            length: 64,
            special: true,
            ..GenOptions::default()
        };
        let pw = generate(&opts).expect("special class must generate");
        assert_eq!(pw.chars().count(), 64);
        assert!(pw.chars().any(|c| SPECIAL.contains(c)));
        let allowed = union(&opts);
        assert!(pw.chars().all(|c| allowed.contains(c)));
    }

    #[test]
    fn too_short_or_no_class_errors() {
        let too_short = GenOptions {
            length: 2,
            ..GenOptions::default()
        };
        assert!(generate(&too_short).is_err());

        let no_class = GenOptions {
            length: 20,
            upper: false,
            lower: false,
            digits: false,
            special: false,
        };
        assert!(generate(&no_class).is_err());
    }

    #[test]
    fn exact_length_small() {
        let opts = GenOptions {
            length: 4,
            upper: false,
            digits: true,
            special: false,
            ..GenOptions::default()
        };
        let pw = generate(&opts).expect("length 4 with two classes must generate");
        assert_eq!(pw.chars().count(), 4);
        let allowed = union(&opts);
        assert!(pw.chars().all(|c| allowed.contains(c)));
    }
}
