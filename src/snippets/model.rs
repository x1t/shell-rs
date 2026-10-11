//! The lines of the snippet list, from the store's snippets and categories.

use std::collections::HashSet;

use crate::host::{Snippet, SnippetCategory, SnippetCategoryId, SnippetId, by_name};

/// A line of the snippet list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnippetRow {
    /// A category's heading, its snippets after it unless `folded`. `None`
    /// is 未分类. `count` is the snippets it lists: while searching, those
    /// that match.
    Category {
        id: Option<SnippetCategoryId>,
        count: usize,
        folded: bool,
    },
    Snippet(SnippetId),
}

/// The list: each category by name with its snippets in the order they
/// were dragged into, and the snippets of none last under 未分类. Without categories there are no
/// headings at all. A search lists what matches, in every category, folded
/// or not, and leaves out the categories with nothing that does.
pub fn rows(
    categories: &[SnippetCategory],
    snippets: &[Snippet],
    query: &str,
    folded: &HashSet<Option<SnippetCategoryId>>,
) -> Vec<SnippetRow> {
    let searching = !query.trim().is_empty();
    let mut matching: Vec<&Snippet> = snippets
        .iter()
        .filter(|snippet| snippet.matches(query))
        .collect();
    matching.sort_by(|a, b| Snippet::in_order(a, b));
    let in_category = |category: Option<SnippetCategoryId>| -> Vec<SnippetId> {
        matching
            .iter()
            .filter(|snippet| snippet.category == category)
            .map(|snippet| snippet.id)
            .collect()
    };
    if categories.is_empty() {
        return in_category(None)
            .into_iter()
            .map(SnippetRow::Snippet)
            .collect();
    }
    let mut sorted: Vec<&SnippetCategory> = categories.iter().collect();
    sorted.sort_by(|a, b| by_name((&a.name, a.id.0), (&b.name, b.id.0)));
    let mut rows = Vec::new();
    let mut section = |id: Option<SnippetCategoryId>, shown_empty: bool| {
        let ids = in_category(id);
        if ids.is_empty() && !shown_empty {
            return;
        }
        let folded = !searching && folded.contains(&id);
        rows.push(SnippetRow::Category {
            id,
            count: ids.len(),
            folded,
        });
        if !folded {
            rows.extend(ids.into_iter().map(SnippetRow::Snippet));
        }
    };
    for category in sorted {
        // A new category shows, empty, until something goes in it.
        section(Some(category.id), !searching);
    }
    section(None, false);
    rows
}

/// A line of the list as drawn: a heading, or up to as many snippets side
/// by side as there are columns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SnippetLine {
    Category {
        id: Option<SnippetCategoryId>,
        count: usize,
        folded: bool,
    },
    Snippets(Vec<SnippetId>),
}

/// `rows` laid out in `columns`: a heading has a line to itself, and the
/// snippets after it fill the lines left to right.
pub fn lines(rows: &[SnippetRow], columns: usize) -> Vec<SnippetLine> {
    let mut lines = Vec::new();
    for row in rows {
        match *row {
            SnippetRow::Category { id, count, folded } => {
                lines.push(SnippetLine::Category { id, count, folded })
            }
            SnippetRow::Snippet(id) => match lines.last_mut() {
                Some(SnippetLine::Snippets(ids)) if ids.len() < columns => ids.push(id),
                _ => lines.push(SnippetLine::Snippets(vec![id])),
            },
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::SnippetDraft;

    fn category(id: u64, name: &str) -> SnippetCategory {
        SnippetCategory::new(SnippetCategoryId(id), name)
    }

    fn snippet(id: u64, name: &str, category: Option<u64>) -> Snippet {
        Snippet::new(
            SnippetId(id),
            SnippetDraft::new(
                name,
                format!("echo {name}"),
                category.map(SnippetCategoryId),
            ),
        )
    }

    fn heading(id: Option<u64>, count: usize, folded: bool) -> SnippetRow {
        SnippetRow::Category {
            id: id.map(SnippetCategoryId),
            count,
            folded,
        }
    }

    fn item(id: u64) -> SnippetRow {
        SnippetRow::Snippet(SnippetId(id))
    }

    #[test]
    fn without_categories_the_snippets_are_listed_by_name() {
        let snippets = [snippet(1, "uptime", None), snippet(2, "df", None)];
        assert_eq!(
            rows(&[], &snippets, "", &HashSet::new()),
            [item(2), item(1)]
        );
    }

    #[test]
    fn categories_by_name_then_the_rest_under_none() {
        let categories = [
            category(1, "日志"),
            category(2, "Docker"),
            category(3, "空"),
        ];
        let snippets = [
            snippet(1, "tail", Some(1)),
            snippet(2, "ps", Some(2)),
            snippet(3, "images", Some(2)),
            snippet(4, "df", None),
        ];
        assert_eq!(
            rows(&categories, &snippets, "", &HashSet::new()),
            [
                heading(Some(2), 2, false),
                item(3),
                item(2),
                heading(Some(1), 1, false),
                item(1),
                heading(Some(3), 0, false),
                heading(None, 1, false),
                item(4),
            ]
        );

        // Folded, a category keeps its heading and count.
        let folded = HashSet::from([Some(SnippetCategoryId(2)), None]);
        assert_eq!(
            rows(&categories, &snippets, "", &folded),
            [
                heading(Some(2), 2, true),
                heading(Some(1), 1, false),
                item(1),
                heading(Some(3), 0, false),
                heading(None, 1, true),
            ]
        );

        // A search opens what it finds and leaves out the rest.
        assert_eq!(
            rows(&categories, &snippets, "S", &folded),
            [heading(Some(2), 2, false), item(3), item(2)]
        );
    }

    #[test]
    fn snippets_keep_the_order_they_were_put_in() {
        let snippets = [
            snippet(1, "uptime", None).with_sort_order(1),
            snippet(2, "df", None).with_sort_order(2),
            snippet(3, "ps", None).with_sort_order(0),
        ];
        assert_eq!(
            rows(&[], &snippets, "", &HashSet::new()),
            [item(3), item(1), item(2)]
        );
    }

    #[test]
    fn in_two_columns_the_snippets_pair_up_under_their_heading() {
        let rows = [
            heading(Some(2), 3, false),
            item(1),
            item(2),
            item(3),
            heading(Some(1), 0, false),
            heading(None, 1, false),
            item(4),
        ];
        let line =
            |ids: &[u64]| SnippetLine::Snippets(ids.iter().map(|id| SnippetId(*id)).collect());
        let title = |id: Option<u64>, count| SnippetLine::Category {
            id: id.map(SnippetCategoryId),
            count,
            folded: false,
        };
        assert_eq!(
            lines(&rows, 2),
            [
                title(Some(2), 3),
                line(&[1, 2]),
                line(&[3]),
                title(Some(1), 0),
                title(None, 1),
                line(&[4]),
            ]
        );
        // One column is a line each.
        assert_eq!(lines(&[item(1), item(2)], 1), [line(&[1]), line(&[2])]);
    }
}
