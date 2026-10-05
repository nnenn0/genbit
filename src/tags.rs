use crate::{
    content::Article,
    route::{UNTAGGED_TAG, tag_url},
};
use anyhow::{Error, ensure};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

/// A tag written in front matter: lowercase kebab-case, and not the reserved `untagged`.
#[derive(Debug, Deserialize)]
#[serde(try_from = "String")]
pub(crate) struct Tag(String);

impl Tag {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Tag {
    type Error = Error;

    fn try_from(tag: String) -> Result<Self, Error> {
        ensure!(
            valid_tag(&tag),
            "invalid tag {tag:?}: use lowercase kebab-case"
        );
        ensure!(
            tag != UNTAGGED_TAG,
            "tag \"untagged\" is reserved for articles without tags"
        );
        Ok(Self(tag))
    }
}

fn valid_tag(tag: &str) -> bool {
    !tag.is_empty()
        && tag.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        })
}

/// The tags of one article, without duplicates.
#[derive(Debug, Default, Deserialize)]
#[serde(try_from = "Vec<Tag>")]
pub(crate) struct Tags(Vec<Tag>);

impl Tags {
    pub(crate) fn into_vec(self) -> Vec<Tag> {
        self.0
    }
}

impl TryFrom<Vec<Tag>> for Tags {
    type Error = Error;

    fn try_from(tags: Vec<Tag>) -> Result<Self, Error> {
        let mut seen = BTreeSet::new();
        for tag in &tags {
            ensure!(
                seen.insert(tag.as_str()),
                "duplicate tag {:?}",
                tag.as_str()
            );
        }
        Ok(Self(tags))
    }
}

/// Returns the tags an article is listed under. Articles without tags are
/// listed under the reserved `untagged` tag.
pub(crate) fn article_tags(article: &Article) -> Vec<&str> {
    if article.tags.is_empty() {
        vec![UNTAGGED_TAG]
    } else {
        article.tags.iter().map(Tag::as_str).collect()
    }
}

/// Articles grouped by tag, ordered by tag name with `untagged` last. Each
/// group keeps the order of the articles it was built from.
pub(crate) struct TagIndex<'a> {
    groups: Vec<TagGroup<'a>>,
}

pub(crate) struct TagGroup<'a> {
    pub(crate) name: &'a str,
    pub(crate) articles: Vec<&'a Article>,
}

impl<'a> TagIndex<'a> {
    pub(crate) fn new(articles: &'a [Article]) -> Self {
        let mut tagged: BTreeMap<&str, Vec<&Article>> = BTreeMap::new();
        for article in articles {
            for tag in article_tags(article) {
                tagged.entry(tag).or_default().push(article);
            }
        }
        // `untagged` sorts among the tag names, but is listed last.
        let untagged = tagged.remove(UNTAGGED_TAG);
        let groups = tagged
            .into_iter()
            .chain(untagged.map(|articles| (UNTAGGED_TAG, articles)))
            .map(|(name, articles)| TagGroup { name, articles })
            .collect();
        Self { groups }
    }

    pub(crate) fn groups(&self) -> &[TagGroup<'a>] {
        &self.groups
    }
}

impl TagGroup<'_> {
    pub(crate) fn url(&self) -> String {
        tag_url(self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::{TagIndex, article_tags};
    use crate::content::{self, Article};
    use anyhow::Result;
    use jiff::tz::TimeZone;
    use std::path::Path;

    fn article(name: &str, tags: &str) -> Result<Article> {
        content::parse(
            &format!(
                "+++\ncreated_at = 2026-09-17 00:00\nupdated_at = 2026-09-17 00:00\ndescription = 'Post'\ntags = {tags}\n+++\n"
            ),
            "content",
            Path::new(&format!("{name}/index.md")),
            &TimeZone::UTC,
            |_, _| Ok(None),
        )
    }

    #[test]
    fn groups_articles_by_tag_with_untagged_last() -> Result<()> {
        let articles = [
            article("a", "['web', 'rust']")?,
            article("b", "[]")?,
            article("c", "['rust']")?,
        ];
        let index = TagIndex::new(&articles);
        let groups = index
            .groups()
            .iter()
            .map(|group| {
                (
                    group.name,
                    group.url(),
                    group
                        .articles
                        .iter()
                        .map(|article| article.route.url())
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            groups,
            [
                ("rust", "/tags/rust/".to_owned(), vec!["/a/", "/c/"]),
                ("web", "/tags/web/".to_owned(), vec!["/a/"]),
                ("untagged", "/tags/untagged/".to_owned(), vec!["/b/"]),
            ]
        );
        let [tagged, untagged, _] = &articles;
        assert_eq!(article_tags(untagged), ["untagged"]);
        assert_eq!(article_tags(tagged), ["web", "rust"]);
        assert!(
            TagIndex::new(std::slice::from_ref(tagged))
                .groups()
                .iter()
                .all(|group| group.name != "untagged")
        );
        Ok(())
    }
}
