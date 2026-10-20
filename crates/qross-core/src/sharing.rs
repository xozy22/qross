//! Rechte je Rechner: Was der Server mit welchem Client teilt (Zwischenablage, kopierte Dateien,
//! Drag & Drop), jeweils mit Richtung. Der Server setzt die Regeln selbst durch; Clients ab
//! Qross 0.7 erfahren ihre Rechte über `Control::Sharing`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub use qross_proto::{Share, Sharing};

/// Rechte aller Rechner eines Servers.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
#[serde(default)]
pub struct SharingRules {
    /// Vorgabe für Rechner ohne eigenen Eintrag, etwa neu hinzugekommene
    pub default: Sharing,
    /// Name des Rechners → was er teilen darf
    pub clients: BTreeMap<String, Sharing>,
}

impl SharingRules {
    /// Was der Server mit `name` teilt: sein Eintrag oder die Vorgabe.
    pub fn for_client(&self, name: &str) -> Sharing {
        self.clients.get(name).copied().unwrap_or(self.default)
    }

    /// Ist irgendetwas eingeschränkt?
    pub fn restricts(&self) -> bool {
        self.default != Sharing::ALL || self.clients.values().any(|s| *s != Sharing::ALL)
    }
}

/// Fürs Log, z. B. „clipboard to the client only, copied files off“; „everything allowed“, wenn
/// nichts eingeschränkt ist.
pub fn describe(sharing: Sharing) -> String {
    let parts: Vec<String> = [("clipboard", sharing.clipboard), ("copied files", sharing.files), ("drag and drop", sharing.drag)]
        .into_iter()
        .filter(|(_, share)| *share != Share::Both)
        .map(|(what, share)| {
            let how = match share {
                Share::Both => "in both directions",
                Share::ToClient => "to the client only",
                Share::FromClient => "from the client only",
                Share::Off => "off",
            };
            format!("{what} {how}")
        })
        .collect();
    if parts.is_empty() {
        "everything allowed".to_owned()
    } else {
        parts.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clients_without_an_entry_get_the_default() {
        let laptop = Sharing { clipboard: Share::ToClient, files: Share::Off, drag: Share::Both };
        let mut rules = SharingRules::default();
        assert!(!rules.restricts());
        assert_eq!(rules.for_client("laptop"), Sharing::ALL);

        rules.clients.insert("laptop".into(), laptop);
        rules.default.drag = Share::Off;
        assert!(rules.restricts());
        assert_eq!(rules.for_client("laptop"), laptop, "eigener Eintrag gilt ganz");
        assert_eq!(rules.for_client("neu").drag, Share::Off, "unbekannte Rechner bekommen die Vorgabe");
        assert_eq!(rules.for_client("neu").clipboard, Share::Both);
    }

    #[test]
    fn descriptions_name_only_restrictions() {
        assert_eq!(describe(Sharing::ALL), "everything allowed");
        let sharing = Sharing { clipboard: Share::ToClient, files: Share::Off, drag: Share::FromClient };
        assert_eq!(describe(sharing), "clipboard to the client only, copied files off, drag and drop from the client only");
    }
}
