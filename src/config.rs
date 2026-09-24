use anyhow::{Context, Result, ensure};
use http::Uri;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    title: String,
    description: String,
    site_url: String,
    og_image: String,
}

#[derive(Serialize)]
pub(crate) struct Config {
    pub(crate) title: String,
    pub(crate) description: String,
    pub(crate) site_url: SiteUrl,
    pub(crate) og_image: String,
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
        Ok(Self {
            title: raw.title,
            description: raw.description,
            site_url,
            og_image,
        })
    }
}

impl SiteUrl {
    fn parse(value: &str) -> Result<Self> {
        ensure!(
            !value.contains('#'),
            "config.toml: site_url must not contain a fragment"
        );
        let uri: Uri = value
            .parse()
            .context("config.toml: site_url must be an absolute HTTP(S) URL")?;
        let scheme = uri
            .scheme_str()
            .context("config.toml: site_url must have a scheme")?;
        ensure!(
            scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https"),
            "config.toml: site_url must use http or https"
        );
        let authority = uri
            .authority()
            .context("config.toml: site_url must have a host")?;
        ensure!(
            !authority.host().is_empty() && !authority.as_str().contains('@'),
            "config.toml: site_url must have a host without credentials"
        );
        ensure!(
            uri.path_and_query().is_none_or(|part| part.as_str() == "/"),
            "config.toml: site_url must point to the site root without a path or query"
        );
        Ok(Self(format!(
            "{}://{authority}/",
            scheme.to_ascii_lowercase()
        )))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn join_root_path(&self, path: &str) -> String {
        format!("{}{path}", self.0.trim_end_matches('/'))
    }
}

fn validate_og_image(value: &str, site_url: &SiteUrl) -> Result<String> {
    ensure!(
        !value.contains('#'),
        "config.toml: og_image must not contain a fragment"
    );
    if value.starts_with('/') {
        ensure!(
            !value.starts_with("//"),
            "config.toml: og_image must be a root-relative path or absolute HTTP(S) URL"
        );
        let uri: Uri = value
            .parse()
            .context("config.toml: og_image must be a valid root-relative path")?;
        ensure!(
            uri.authority().is_none() && uri.scheme().is_none(),
            "config.toml: og_image must be a root-relative path or absolute HTTP(S) URL"
        );
        return Ok(site_url.join_root_path(&uri.to_string()));
    }

    let uri: Uri = value
        .parse()
        .context("config.toml: og_image must be a root-relative path or absolute HTTP(S) URL")?;
    let scheme = uri
        .scheme_str()
        .context("config.toml: og_image must be an absolute HTTP(S) URL")?;
    ensure!(
        scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https"),
        "config.toml: og_image must use http or https"
    );
    let authority = uri
        .authority()
        .context("config.toml: og_image must have a host")?;
    ensure!(
        !authority.host().is_empty() && !authority.as_str().contains('@'),
        "config.toml: og_image must have a host without credentials"
    );
    Ok(value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::Config;
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
}
