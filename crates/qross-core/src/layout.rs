//! Anordnung der Bildschirme: welcher Rechner an welcher Kante liegt.

use std::collections::HashMap;

use qross_proto::Edge;
use serde::Deserialize;

/// Eintrag aus der Konfiguration: `from` hat an `edge` den Nachbarn `to`.
/// Die Gegenrichtung wird automatisch ergänzt, sofern sie nicht explizit anders belegt ist.
#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct LinkConfig {
    pub from: String,
    pub edge: Edge,
    pub to: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum LayoutError {
    #[error("'{0}' cannot be its own neighbor")]
    SelfLink(String),
    #[error("edge {edge:?} of '{screen}' has two neighbors ('{first}' and '{second}')")]
    Conflict { screen: String, edge: Edge, first: String, second: String },
}

#[derive(Debug, Clone, Default)]
pub struct Layout {
    neighbors: HashMap<String, [Option<String>; 4]>,
}

impl Layout {
    pub fn from_links(links: &[LinkConfig]) -> Result<Self, LayoutError> {
        let mut layout = Layout::default();
        for link in links {
            if link.from == link.to {
                return Err(LayoutError::SelfLink(link.from.clone()));
            }
            let slot = &mut layout.slots(&link.from)[link.edge.index()];
            match slot {
                Some(existing) if *existing != link.to => {
                    return Err(LayoutError::Conflict {
                        screen: link.from.clone(),
                        edge: link.edge,
                        first: existing.clone(),
                        second: link.to.clone(),
                    })
                }
                _ => *slot = Some(link.to.clone()),
            }
        }
        for link in links {
            let back = &mut layout.slots(&link.to)[link.edge.opposite().index()];
            if back.is_none() {
                *back = Some(link.from.clone());
            }
        }
        Ok(layout)
    }

    fn slots(&mut self, screen: &str) -> &mut [Option<String>; 4] {
        self.neighbors.entry(screen.to_owned()).or_default()
    }

    pub fn neighbor(&self, screen: &str, edge: Edge) -> Option<&str> {
        self.neighbors.get(screen)?[edge.index()].as_deref()
    }

    pub fn contains(&self, screen: &str) -> bool {
        self.neighbors.contains_key(screen)
    }

    pub fn screens(&self) -> impl Iterator<Item = &str> {
        self.neighbors.keys().map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(from: &str, edge: Edge, to: &str) -> LinkConfig {
        LinkConfig { from: from.into(), edge, to: to.into() }
    }

    #[test]
    fn reverse_links_are_added() {
        let l = Layout::from_links(&[link("pc", Edge::Right, "laptop")]).unwrap();
        assert_eq!(l.neighbor("pc", Edge::Right), Some("laptop"));
        assert_eq!(l.neighbor("laptop", Edge::Left), Some("pc"));
        assert_eq!(l.neighbor("pc", Edge::Left), None);
        assert!(l.contains("laptop"));
    }

    #[test]
    fn explicit_reverse_link_wins() {
        // Wrap-around: laptop führt rechts wieder zum pc, links zum tablet
        let l = Layout::from_links(&[
            link("pc", Edge::Right, "laptop"),
            link("laptop", Edge::Left, "tablet"),
        ])
        .unwrap();
        assert_eq!(l.neighbor("laptop", Edge::Left), Some("tablet"));
        assert_eq!(l.neighbor("tablet", Edge::Right), Some("laptop"));
    }

    #[test]
    fn conflicts_and_self_links_are_rejected() {
        let conflict = Layout::from_links(&[link("pc", Edge::Right, "a"), link("pc", Edge::Right, "b")]).unwrap_err();
        assert!(matches!(conflict, LayoutError::Conflict { .. }));
        assert_eq!(conflict.to_string(), "edge Right of 'pc' has two neighbors ('a' and 'b')");
        let own = Layout::from_links(&[link("pc", Edge::Top, "pc")]).unwrap_err();
        assert_eq!(own, LayoutError::SelfLink("pc".into()));
        assert_eq!(own.to_string(), "'pc' cannot be its own neighbor");
    }
}
