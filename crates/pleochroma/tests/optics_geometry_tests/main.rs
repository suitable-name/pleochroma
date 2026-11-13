//! Integration tests for `pleochroma`'s Fresnel/Mueller math, facet-angle classification,
//! B-Rep reconstruction and real `.asc`-design geometry.
//!
//! Each topic is a sibling module below; `fixtures` holds helpers shared between them.

mod asc_designs;
mod brep_reconstruction;
mod facet_angle_classification;
mod fixtures;
mod fresnel_mueller;
