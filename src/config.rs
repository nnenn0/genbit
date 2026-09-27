use anyhow::{Context, Result, ensure};
use http::Uri;
use jiff::tz::TimeZone;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    title: String,
    description: String,
    site_url: String,
    og_image: String,
    timezone: String,
}

#[derive(Serialize)]
pub(crate) struct Config {
    pub(crate) title: String,
    pub(crate) description: String,
    pub(crate) site_url: SiteUrl,
    pub(crate) og_image: String,
    #[serde(skip)]
    pub(crate) timezone: TimeZone,
}

#[derive(Serialize)]
#[serde(transparent)]
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
        let timezone = TimeZone::get(&raw.timezone).with_context(|| {
            format!(
                "config.toml: timezone must be an IANA time zone name such as Asia/Tokyo: {}",
                raw.timezone
            )
        })?;
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
    fn accepts_iana_time_zone_names_without_publishing_them() -> Result<()> {
        let config = Config::parse(
            "title = 'Blog'\ndescription = 'Posts'\nsite_url = 'https://example.com/'\nog_image = '/images/card.png'\ntimezone = 'America/New_York'\n",
        )?;
        assert_eq!(config.timezone.iana_name(), Some("America/New_York"));
        let published = serde_json::to_value(config)?;
        assert!(published.get("timezone").is_none());
        Ok(())
    }
}
