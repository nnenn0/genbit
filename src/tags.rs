use crate::{
    content::Article,
    route::{UNTAGGED_TAG, tag_url},
};
use std::collections::BTreeMap;

/// Returns the tags an article is listed under. Articles without tags are
/// listed under the reserved `untagged` tag.
pub(crate) fn article_tags(article: &Article) -> Vec<&str> {
    if article.tags.is_empty() {
        vec![UNTAGGED_TAG]
    } else {
        article.tags.iter().map(String::as_str).collect()
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
        let mut untagged = Vec::new();
        for article in articles {
            if article.tags.is_empty() {
                untagged.push(article);
            }
            for tag in &article.tags {
                tagged.entry(tag).or_default().push(article);
            }
        }
        let mut groups = tagged
            .into_iter()
            .map(|(name, articles)| TagGroup { name, articles })
            .collect::<Vec<_>>();
        if !untagged.is_empty() {
            groups.push(TagGroup {
                name: UNTAGGED_TAG,
                articles: untagged,
            });
        }
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
            Path::new(&format!("{name}.md")),
            &TimeZone::UTC,
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
                ("rust", "/tags/rust/".to_owned(), vec!["/a", "/c"]),
                ("web", "/tags/web/".to_owned(), vec!["/a"]),
                ("untagged", "/tags/untagged/".to_owned(), vec!["/b"]),
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
