use std::{
    io::{self, Write},
    net::IpAddr,
    process::ExitCode,
};

fn hostname(value: &str) -> bool {
    let name = value.strip_suffix('.').unwrap_or(value);
    name.len() <= 253
        && name.bytes().any(|b| b.is_ascii_alphabetic())
        && name.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

fn directives(value: &str) -> Option<String> {
    if value.trim().is_empty() {
        return Some("# No TRUSTED_PROXIES configured\n".to_owned());
    }
    let mut config = "real_ip_header X-Forwarded-For;\nreal_ip_recursive on;\n".to_owned();
    for entry in value.split(',').map(str::trim) {
        let (address, prefix) = entry
            .split_once('/')
            .map_or((entry, None), |(ip, prefix)| (ip, Some(prefix)));
        if let Some(prefix) = prefix {
            let ip: IpAddr = address.parse().ok()?;
            let max = if ip.is_ipv4() { 32 } else { 128 };
            if prefix.is_empty()
                || !prefix.bytes().all(|b| b.is_ascii_digit())
                || prefix.parse::<u8>().ok()? > max
            {
                return None;
            }
        } else if address.parse::<IpAddr>().is_err() && !hostname(address) {
            return None;
        }
        config.push_str("set_real_ip_from ");
        config.push_str(entry);
        config.push_str(";\n");
    }
    Some(config)
}

fn main() -> ExitCode {
    let value = std::env::var("TRUSTED_PROXIES");
    let value = match &value {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent) => "",
        Err(_) => return ExitCode::FAILURE,
    };
    let Some(config) = directives(value) else {
        eprintln!(
            "Invalid TRUSTED_PROXIES: expected comma-separated IP addresses, CIDRs, or hostnames"
        );
        return ExitCode::FAILURE;
    };
    if io::stdout().write_all(config.as_bytes()).is_err() {
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_explicit_hostname_entries() {
        let config = directives("localhost, proxy.example.test.").unwrap();
        assert!(
            config
                .ends_with("set_real_ip_from localhost;\nset_real_ip_from proxy.example.test.;\n")
        );
    }

    #[test]
    fn only_explicit_ip_addresses_and_cidrs_are_emitted() {
        assert!(!directives(" ").unwrap().contains("set_real_ip_from"));
        assert_eq!(
            directives(" 127.0.0.1/32, 2001:db8::/32, ::1 ").unwrap(),
            "real_ip_header X-Forwarded-For;\nreal_ip_recursive on;\nset_real_ip_from 127.0.0.1/32;\nset_real_ip_from 2001:db8::/32;\nset_real_ip_from ::1;\n"
        );
        for input in [
            "127.0.0.1/33",
            "::1/129",
            "127.0.0.1/",
            "localhost:80",
            "localhost/24",
            "-proxy.example.test",
            "proxy..example.test",
            "*.example.test",
            "127.000.0.1",
            "unix:",
            "127.0.0.1,",
            "127.0.0.1 ;",
            "::1\n::2",
        ] {
            assert!(directives(input).is_none(), "{input:?}");
        }
    }
}
