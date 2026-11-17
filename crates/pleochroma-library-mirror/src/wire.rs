//! Conversions between the library protocol's wire types and the vault's stored forms.

use pleochroma_net::library::FrostedTierWire;
use pleochroma_vault::model::frosting::StoredTierFrosting;

/// Converts a design's frosting from the library protocol's form to the stored form.
///
/// The wire form ([`FrostedTierWire`], on
/// `DesignRecord::frosting` and `LibraryResponse::DesignSource::frosting`) in the form the
/// local library stores (`Database::set_diagram_frosting`) and the editor applies
/// (`StoredTierFrosting::to_gcs`, then `Design::apply_imported_frosting`).
#[must_use]
pub fn frosting_from_wire(frosting: &[FrostedTierWire]) -> Vec<StoredTierFrosting> {
    frosting
        .iter()
        .map(|row| StoredTierFrosting {
            tier: row.tier,
            facets: row.facets.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wire_frosting_becomes_the_stored_rows_unchanged() {
        let wire = vec![
            FrostedTierWire {
                tier: 0,
                facets: Vec::new(),
            },
            FrostedTierWire {
                tier: 4,
                facets: vec![2, 5],
            },
        ];
        assert_eq!(
            frosting_from_wire(&wire),
            vec![
                StoredTierFrosting::whole(0),
                StoredTierFrosting {
                    tier: 4,
                    facets: vec![2, 5]
                },
            ]
        );
        assert_eq!(
            frosting_from_wire(&[]),
            [] as [pleochroma_vault::model::frosting::StoredTierFrosting; 0]
        );
    }
}
