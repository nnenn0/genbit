use crate::text::PublishableText;
use anyhow::{Context, Result, ensure};
use http::Uri;
use jiff::tz::TimeZone;
use serde::Deserialize;
use std::net::{Ipv4Addr, Ipv6Addr};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    title: String,
    description: String,
    site_url: String,
    og_image: String,
    timezone: String,
}

pub(crate) struct Config {
    pub(crate) title: PublishableText,
    pub(crate) description: PublishableText,
    pub(crate) site_url: SiteUrl,
    pub(crate) og_image: String,
    pub(crate) timezone: TimeZone,
}

pub(crate) struct SiteUrl(String);

impl Config {
    pub(crate) fn parse(source: &str) -> Result<Self> {
        let raw: RawConfig = toml::from_str(source)?;
        raw.try_into()
    }
}

impl TryFrom<RawConfig> for Config {
    type Error = anyhow::Error;

    fn try_from(raw: RawConfig) -> Result<Self> {
        let title = PublishableText::new(raw.title, "config.toml: title")?;
        let description = PublishableText::new(raw.description, "config.toml: description")?;
        let site_url = SiteUrl::parse(&raw.site_url)?;
        let og_image = og_image_url(&raw.og_image, &site_url)?;
        let timezone = TimeZone::get(&raw.timezone).with_context(|| {
            format!(
                "config.toml: timezone must be an IANA time zone name such as Asia/Tokyo: {}",
                raw.timezone
            )
        })?;
        Ok(Self {
            title,
            description,
            site_url,
            og_image,
            timezone,
        })
    }
}

impl SiteUrl {
    fn parse(value: &str) -> Result<Self> {
        let url = parse_http_url(value, "site_url")?;
        ensure!(
            url.uri
                .path_and_query()
                .is_none_or(|part| part.as_str() == "/"),
            "config.toml: site_url must point to the site root without a path or query"
        );
        Ok(Self(format!("{}/", url.origin)))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn join_root_path(&self, path: &str) -> String {
        format!("{}{path}", self.0.trim_end_matches('/'))
    }
}

/// Returns `og_image` as an absolute URL, joining a root-relative path to `site_url`.
fn og_image_url(value: &str, site_url: &SiteUrl) -> Result<String> {
    if !value.starts_with('/') || value.starts_with("//") {
        parse_http_url(value, "og_image")?;
        return Ok(value.to_owned());
    }
    ensure!(
        !value.contains('#'),
        "config.toml: og_image must not contain a fragment"
    );
    let uri: Uri = value
        .parse()
        .context("config.toml: og_image must be a valid root-relative path")?;
    Ok(site_url.join_root_path(&uri.to_string()))
}

struct HttpUrl {
    uri: Uri,
    /// Lowercase scheme and authority, such as `https://example.com`.
    origin: String,
}

/// Parses an absolute HTTP(S) URL with a host and no credentials or fragment.
fn parse_http_url(value: &str, field: &str) -> Result<HttpUrl> {
    ensure!(
        !value.contains('#'),
        "config.toml: {field} must not contain a fragment"
    );
    let uri: Uri = value
        .parse()
        .with_context(|| format!("config.toml: {field} must be an absolute HTTP(S) URL"))?;
    let scheme = uri
        .scheme_str()
        .with_context(|| format!("config.toml: {field} must be an absolute HTTP(S) URL"))?;
    ensure!(
        scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https"),
        "config.toml: {field} must use http or https"
    );
    let authority = uri
        .authority()
        .with_context(|| format!("config.toml: {field} must have a host"))?;
    ensure!(
        !authority.host().is_empty() && !authority.as_str().contains('@'),
        "config.toml: {field} must have a host without credentials"
    );
    ensure!(
        is_valid_host(authority.host()),
        "config.toml: {field} must have a valid host name or IP address: {}",
        authority.host()
    );
    // `Uri` accepts any text after the colon, but only a port number is usable.
    let port = authority
        .as_str()
        .strip_prefix(authority.host())
        .unwrap_or(authority.as_str());
    ensure!(
        port.is_empty() || authority.port_u16().is_some_and(|number| number != 0),
        "config.toml: {field} must have a port number from 1 to 65535: {port}"
    );
    let origin = format!("{}://{authority}", scheme.to_ascii_lowercase());
    Ok(HttpUrl { uri, origin })
}

fn is_valid_host(host: &str) -> bool {
    if let Some(address) = host
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
    {
        return address.parse::<Ipv6Addr>().is_ok();
    }
    // Numeric hosts must be dotted-decimal IPv4, not unchecked DNS labels or
    // shorthand forms whose interpretation differs between URL consumers.
    if host
        .bytes()
        .all(|byte| byte.is_ascii_digit() || byte == b'.')
    {
        return host.parse::<Ipv4Addr>().is_ok();
    }
    host.len() <= 253
        && host.split('.').all(|label| {
            (1..=63).contains(&label.len())
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

#[cfg(test)]
mod tests {
    use super::Config;
    use anyhow::{Context, Result};

    #[test]
    fn normalizes_site_url_and_resolves_root_relative_image() -> Result<()> {
        let config = Config::parse(
            "title = 'Blog'\ndescription = 'Posts'\nsite_url = 'HTTPS://example.com'\nog_image = '/images/card.png?size=large'\ntimezone = 'Asia/Tokyo'\n",
        )?;
        assert_eq!(config.site_url.as_str(), "https://example.com/");
        assert_eq!(
            config.site_url.join_root_path("/entries/post"),
            "https://example.com/entries/post"
        );
        assert_eq!(
            config.og_image,
            "https://example.com/images/card.png?size=large"
        );
        assert_eq!(config.site_url.as_str(), "https://example.com/");
        Ok(())
    }

    #[test]
    fn rejects_missing_unknown_and_invalid_config_values() -> Result<()> {
        let valid = "title = 'Blog'\ndescription = 'Posts'\nsite_url = 'https://example.com/'\nog_image = '/images/card.png'\ntimezone = 'Asia/Tokyo'\n";
        for (source, reason) in [
            (valid.replace("title = 'Blog'\n", ""), "title"),
            (valid.replace("description = 'Posts'\n", ""), "description"),
            (
                valid.replace("site_url = 'https://example.com/'\n", ""),
                "site_url",
            ),
            (
                valid.replace("og_image = '/images/card.png'\n", ""),
                "og_image",
            ),
            (valid.replace("timezone = 'Asia/Tokyo'\n", ""), "timezone"),
            (valid.replace("title = 'Blog'", "title = '  '"), "title"),
            (
                valid.replace("description = 'Posts'", "description = '  '"),
                "description",
            ),
            (
                valid.replace("title = 'Blog'", "title = \"a\\u0001b\""),
                "title must not contain control characters",
            ),
            (
                valid.replace("description = 'Posts'", "description = \"a\\u007Fb\""),
                "description must not contain control characters",
            ),
            (format!("{valid}unknown = true\n"), "unknown"),
            (
                valid.replace("https://example.com/", "https://example.com/blog/"),
                "site_url",
            ),
            (
                valid.replace("https://example.com/", "https://example.com/?q=1"),
                "site_url",
            ),
            (
                valid.replace("https://example.com/", "https://example.com:abc/"),
                "port",
            ),
            (
                valid.replace("https://example.com/", "https://example.com:99999/"),
                "port",
            ),
            (
                valid.replace("https://example.com/", "https://example.com:0/"),
                "port",
            ),
            (
                valid.replace("https://example.com/", "https://example.com:/"),
                "port",
            ),
            (
                valid.replace("https://example.com/", "https://exa_mple.com/"),
                "host",
            ),
            (
                valid.replace("https://example.com/", "https://-example.com/"),
                "host",
            ),
            (
                valid.replace("https://example.com/", "https://example..com/"),
                "host",
            ),
            (
                valid.replace("https://example.com/", "https://[::g]/"),
                "host",
            ),
            (
                valid.replace("/images/card.png", "https://example.com:99999/card.png"),
                "og_image",
            ),
            (
                valid.replace("/images/card.png", "//example.com/card.png"),
                "og_image",
            ),
            (
                valid.replace("/images/card.png", "ftp://example.com/card.png"),
                "og_image",
            ),
            (valid.replace("'Asia/Tokyo'", "'+09:00'"), "timezone"),
            (valid.replace("'Asia/Tokyo'", "'Tokyo'"), "timezone"),
            (valid.replace("'Asia/Tokyo'", "'Asia/Nowhere'"), "timezone"),
            (valid.replace("'Asia/Tokyo'", "''"), "timezone"),
            (valid.replace("'Asia/Tokyo'", "9"), "timezone"),
        ] {
            let error = Config::parse(&source)
                .err()
                .with_context(|| format!("accepted {source:?}"))?;
            assert!(
                format!("{error:#}").contains(reason),
                "{source:?}: {error:#}"
            );
        }
        Ok(())
    }

    #[test]
    fn rejects_invalid_numeric_hosts_in_site_and_image_urls() -> Result<()> {
        for host in [
            "999.999.999.999",
            "256.0.0.1",
            "127.0.0.01",
            "127.1",
            "2130706433",
            "1.2.3.4.5",
        ] {
            for field in ["site_url", "og_image"] {
                let (site_url, og_image) = if field == "site_url" {
                    (format!("https://{host}:443/"), "/card.png".to_owned())
                } else {
                    (
                        "https://example.com/".to_owned(),
                        format!("https://{host}/card.png"),
                    )
                };
                let error = Config::parse(&format!(
                    "title = 'Blog'\ndescription = 'Posts'\nsite_url = '{site_url}'\nog_image = '{og_image}'\ntimezone = 'UTC'\n"
                ))
                .err()
                .with_context(|| format!("accepted {field}: {host}"))?;
                let message = format!("{error:#}");
                assert!(
                    message.contains(field) && message.contains("host"),
                    "{message}"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn accepts_host_names_ip_addresses_and_ports() -> Result<()> {
        for (site_url, expected) in [
            ("http://127.0.0.1:3000", "http://127.0.0.1:3000/"),
            ("http://0.0.0.0:3000", "http://0.0.0.0:3000/"),
            ("https://255.255.255.255", "https://255.255.255.255/"),
            ("https://123.example.com", "https://123.example.com/"),
            ("http://[::1]:8080/", "http://[::1]:8080/"),
            (
                "https://sub-1.example.com:443/",
                "https://sub-1.example.com:443/",
            ),
            ("https://localhost", "https://localhost/"),
        ] {
            let config = Config::parse(&format!(
                "title = 'Blog'\ndescription = 'Posts'\nsite_url = '{site_url}'\nog_image = '/card.png'\ntimezone = 'Asia/Tokyo'\n"
            ))?;
            assert_eq!(config.site_url.as_str(), expected);
        }
        Ok(())
    }

    #[test]
    fn accepts_iana_time_zone_names() -> Result<()> {
        let config = Config::parse(
            "title = 'Blog'\ndescription = 'Posts'\nsite_url = 'https://example.com/'\nog_image = '/images/card.png'\ntimezone = 'America/New_York'\n",
        )?;
        assert_eq!(config.timezone.iana_name(), Some("America/New_York"));
        Ok(())
    }
}
