//! Port of libalpm's `alpm_pkg_vercmp` (rpm-style version comparison).

use std::cmp::Ordering;

fn split_evr(s: &str) -> (&str, &str, Option<&str>) {
    let (epoch, rest) = match s.find(':') {
        Some(i) if s[..i].bytes().all(|b| b.is_ascii_digit()) && i > 0 => (&s[..i], &s[i + 1..]),
        _ => ("0", s),
    };
    match rest.rfind('-') {
        Some(i) => (epoch, &rest[..i], Some(&rest[i + 1..])),
        None => (epoch, rest, None),
    }
}

fn rpmvercmp(a: &str, b: &str) -> Ordering {
    if a == b {
        return Ordering::Equal;
    }
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let (mut i, mut j) = (0usize, 0usize);
    while i < a.len() && j < b.len() {
        let (s1, s2) = (i, j);
        while i < a.len() && !a[i].is_ascii_alphanumeric() {
            i += 1;
        }
        while j < b.len() && !b[j].is_ascii_alphanumeric() {
            j += 1;
        }
        if i >= a.len() || j >= b.len() {
            break;
        }
        if i - s1 != j - s2 {
            return if i - s1 < j - s2 {
                Ordering::Less
            } else {
                Ordering::Greater
            };
        }
        let (st1, st2) = (i, j);
        let isnum = a[i].is_ascii_digit();
        if isnum {
            while i < a.len() && a[i].is_ascii_digit() {
                i += 1;
            }
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
        } else {
            while i < a.len() && a[i].is_ascii_alphabetic() {
                i += 1;
            }
            while j < b.len() && b[j].is_ascii_alphabetic() {
                j += 1;
            }
        }
        if st2 == j {
            // segment types differ: numeric is newer than alpha
            return if isnum {
                Ordering::Greater
            } else {
                Ordering::Less
            };
        }
        let (mut x, mut y) = (&a[st1..i], &b[st2..j]);
        if isnum {
            while x.len() > 1 && x[0] == b'0' {
                x = &x[1..];
            }
            while y.len() > 1 && y[0] == b'0' {
                y = &y[1..];
            }
            if x.len() != y.len() {
                return x.len().cmp(&y.len());
            }
        }
        match x.cmp(y) {
            Ordering::Equal => {}
            o => return o,
        }
    }
    let (ea, eb) = (i >= a.len(), j >= b.len());
    if ea && eb {
        return Ordering::Equal;
    }
    if (ea && !b[j].is_ascii_alphabetic()) || (!ea && a[i].is_ascii_alphabetic()) {
        Ordering::Less
    } else {
        Ordering::Greater
    }
}

/// Compare two full pacman version strings (`[epoch:]version[-release]`).
pub fn vercmp(a: &str, b: &str) -> Ordering {
    if a == b {
        return Ordering::Equal;
    }
    let (e1, v1, r1) = split_evr(a);
    let (e2, v2, r2) = split_evr(b);
    match rpmvercmp(e1, e2) {
        Ordering::Equal => {}
        o => return o,
    }
    match rpmvercmp(v1, v2) {
        Ordering::Equal => {}
        o => return o,
    }
    match (r1, r2) {
        (Some(x), Some(y)) => rpmvercmp(x, y),
        _ => Ordering::Equal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Ordering::*;

    #[test]
    fn basic_ordering() {
        assert_eq!(vercmp("1.0-1", "1.0-1"), Equal);
        assert_eq!(vercmp("1.0-1", "1.0-2"), Less);
        assert_eq!(vercmp("1.10-1", "1.9-1"), Greater);
        assert_eq!(vercmp("1:0.1-1", "2.0-1"), Greater);
        assert_eq!(vercmp("1.0a-1", "1.0-1"), Less);
        assert_eq!(vercmp("1.0", "1.0-5"), Equal);
        assert_eq!(vercmp("1.0.1", "1.0"), Greater);
        assert_eq!(vercmp("2.0rc1-1", "2.0-1"), Less);
        assert_eq!(vercmp("1-64", "1-9"), Greater);
    }
}
