fn number(value: &str, allow_zero: bool) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && (!value.starts_with('0') || (allow_zero && value == "0"))
}

pub fn resolve(base: &str, override_version: Option<&str>) -> Result<String, &'static str> {
    let parts: Vec<_> = base.split('.').collect();
    if parts.len() != 3 || parts.iter().any(|part| !number(part, true)) {
        return Err("Cargo version must be a base version");
    }
    let version = override_version.unwrap_or(base);
    if version == base {
        return Ok(version.to_owned());
    }
    let suffix = version
        .strip_prefix(&format!("{base}-nightly."))
        .ok_or("Build version must match the Cargo base version")?;
    let (date, sequence) = suffix.split_once('.').ok_or("Invalid nightly version")?;
    if date.len() != 8 || !date.bytes().all(|byte| byte.is_ascii_digit()) || !number(sequence, false) {
        return Err("Invalid nightly version");
    }
    let year: u32 = date[..4].parse().unwrap();
    let month: usize = date[4..6].parse().unwrap();
    let day: u32 = date[6..].parse().unwrap();
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if !(1..=12).contains(&month) || day == 0 || day > days[month - 1] {
        return Err("Invalid nightly date");
    }
    Ok(version.to_owned())
}

#[cfg(test)]
mod tests {
    use super::resolve;

    #[test]
    fn stable_and_nightly_keep_the_cargo_base() {
        assert_eq!(resolve("0.2.0", None).unwrap(), "0.2.0");
        assert_eq!(resolve("0.2.0", Some("0.2.0")).unwrap(), "0.2.0");
        assert_eq!(
            resolve("0.2.0", Some("0.2.0-nightly.20261022.123")).unwrap(),
            "0.2.0-nightly.20261022.123"
        );
        for version in [
            "0.3.0",
            "0.3.0-nightly.20261022.1",
            "0.2.0-rc.1",
            "0.2.0-nightly.20260230.1",
            "0.2.0-nightly.20261022.01",
            "0.2.0\nINJECT=1",
        ] {
            assert!(resolve("0.2.0", Some(version)).is_err(), "{version}");
        }
    }
}
