use anyhow::{Context, Result, bail, ensure};
use http::Uri;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    title: String,
    description: String,
    site_url: String,
    og_image: String,
    timezone: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct Config {
    pub(crate) title: String,
    pub(crate) description: String,
    pub(crate) site_url: SiteUrl,
    pub(crate) og_image: String,
    #[serde(skip)]
    pub(crate) timezone: UtcOffset,
}

#[derive(Serialize)]
#[serde(transparent)]
pub(crate) struct SiteUrl(String);

/// Offset applied to article date-times, which are written without a time zone.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct UtcOffset {
    minutes: i16,
}

impl Config {
    pub(crate) fn parse(source: &str) -> Result<Self> {
        let raw: RawConfig = toml::from_str(source)?;
        raw.try_into()
    }
}

impl TryFrom<RawConfig> for Config {
    type Error = anyhow::Error;

    fn try_from(raw: RawConfig) -> Result<Self> {
        ensure!(
            !raw.title.trim().is_empty(),
            "config.toml: title must not be empty"
        );
        ensure!(
            !raw.description.trim().is_empty(),
            "config.toml: description must not be empty"
        );
        let site_url = SiteUrl::parse(&raw.site_url)?;
        let og_image = validate_og_image(&raw.og_image, &site_url)?;
        let timezone = raw
            .timezone
            .as_deref()
            .map(UtcOffset::parse)
            .transpose()?
            .unwrap_or_default();
        Ok(Self {
            title: raw.title,
            description: raw.description,
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

impl UtcOffset {
    fn parse(value: &str) -> Result<Self> {
        const FORMAT: &str = "config.toml: timezone must be a UTC offset such as +09:00 or -05:00";
        let bytes = value.as_bytes();
        let [sign, h1, h2, b':', m1, m2] = bytes else {
            bail!(FORMAT);
        };
        let sign: i16 = match sign {
            b'+' => 1,
            b'-' => -1,
            _ => bail!(FORMAT),
        };
        let digit = |byte: &u8| {
            byte.is_ascii_digit()
                .then(|| i16::from(byte - b'0'))
                .context(FORMAT)
        };
        let hours = digit(h1)? * 10 + digit(h2)?;
        let minutes = digit(m1)? * 10 + digit(m2)?;
        ensure!(hours <= 23 && minutes <= 59, FORMAT);
        Ok(Self {
            minutes: sign * (hours * 60 + minutes),
        })
    }

    /// Formats the offset as used by RFC 822 dates, such as `+0900`.
    pub(crate) fn rfc822(self) -> String {
        let sign = if self.minutes < 0 { '-' } else { '+' };
        let minutes = self.minutes.unsigned_abs();
        format!("{sign}{:02}{:02}", minutes / 60, minutes % 60)
    }
}

fn validate_og_image(value: &str, site_url: &SiteUrl) -> Result<String> {
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
    let origin = format!("{}://{authority}", scheme.to_ascii_lowercase());
    Ok(HttpUrl { uri, origin })
}

#[cfg(test)]
mod tests {
    use super::{Config, UtcOffset};
    use anyhow::{Context, Result};

    #[test]
    fn normalizes_site_url_and_resolves_root_relative_image() -> Result<()> {
        let config = Config::parse(
            "title = 'Blog'\ndescription = 'Posts'\nsite_url = 'HTTPS://example.com'\nog_image = '/images/card.png?size=large'\n",
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
        let published = serde_json::to_value(&config)?;
        assert_eq!(
            published
                .get("site_url")
                .and_then(serde_json::Value::as_str),
            Some("https://example.com/")
        );
        assert_eq!(
            published
                .get("og_image")
                .and_then(serde_json::Value::as_str),
            Some(config.og_image.as_str())
        );
        Ok(())
    }

    #[test]
    fn rejects_missing_unknown_and_invalid_config_values() -> Result<()> {
        let valid = "title = 'Blog'\ndescription = 'Posts'\nsite_url = 'https://example.com/'\nog_image = '/images/card.png'\n";
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
            (valid.replace("title = 'Blog'", "title = '  '"), "title"),
            (
                valid.replace("description = 'Posts'", "description = '  '"),
                "description",
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
                valid.replace("/images/card.png", "//example.com/card.png"),
                "og_image",
            ),
            (
                valid.replace("/images/card.png", "ftp://example.com/card.png"),
                "og_image",
            ),
            (format!("{valid}timezone = '+9:00'\n"), "timezone"),
            (format!("{valid}timezone = '+24:00'\n"), "timezone"),
            (format!("{valid}timezone = '+09:60'\n"), "timezone"),
            (format!("{valid}timezone = 'Asia/Tokyo'\n"), "timezone"),
            (format!("{valid}timezone = 9\n"), "timezone"),
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
    fn timezone_defaults_to_utc_and_formats_rfc822_offsets() -> Result<()> {
        let valid = "title = 'Blog'\ndescription = 'Posts'\nsite_url = 'https://example.com/'\nog_image = '/images/card.png'\n";
        assert_eq!(Config::parse(valid)?.timezone, UtcOffset::default());
        assert_eq!(UtcOffset::default().rfc822(), "+0000");
        for (value, expected) in [
            ("+09:00", "+0900"),
            ("-05:30", "-0530"),
            ("-00:00", "+0000"),
        ] {
            let config = Config::parse(&format!("{valid}timezone = '{value}'\n"))?;
            assert_eq!(config.timezone.rfc822(), expected);
        }
        let published = serde_json::to_value(Config::parse(valid)?)?;
        assert!(published.get("timezone").is_none());
        Ok(())
    }
}
