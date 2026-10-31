use super::Row;

// The four newer shapes. Every table below was fitted (and its 4-decimal masts
// re-checked) so that, on the template's default preform and after a remap to each
// gear it offers: every facet reaches the surface, the smallest facet is well above
// the manufacturability threshold, no preform plane cuts the stone, the girdle is
// alive and the outline has the intended length-to-width ratio. Edit a mast by hand
// and the per-template tests in this file tell you what broke.

/// Oval, length-to-width 1.35, 2-fold with mirror. Girdle masts follow the ellipse
/// `hypot(cos a, 1.35 sin a)` around the 16 girdle positions; crown 35 / pavilion 41
/// degrees, the corundum-like figures. Stored in cutting order: the pavilion mains, the
/// girdle, the lower girdle and the culet, then the crown mains, the stars and the upper
/// girdle, the table last.
pub(super) const OVAL_ROWS: &[Row] = &[
    ("Pavilion Main 1", -41.0, &[0.0, 48.0], 0.6787),
    ("Pavilion Main 2", -41.0, &[12.0, 36.0, 60.0, 84.0], 0.802),
    ("Pavilion Main 3", -41.0, &[24.0, 72.0], 0.9083),
    ("Girdle 1", 90.0, &[0.0, 48.0], 1.0),
    ("Girdle 2", 90.0, &[6.0, 42.0, 54.0, 90.0], 1.0585),
    ("Girdle 3", 90.0, &[12.0, 36.0, 60.0, 84.0], 1.188),
    ("Girdle 4", 90.0, &[18.0, 30.0, 66.0, 78.0], 1.3046),
    ("Girdle 5", 90.0, &[24.0, 72.0], 1.35),
    ("Lower Girdle 1", -42.0, &[1.0, 47.0, 49.0, 95.0], 0.695),
    ("Lower Girdle 2", -42.0, &[11.0, 37.0, 59.0, 85.0], 0.7905),
    ("Lower Girdle 3", -42.0, &[13.0, 35.0, 61.0, 83.0], 0.8405),
    ("Lower Girdle 4", -42.0, &[23.0, 25.0, 71.0, 73.0], 0.9237),
    ("Culet", -0.0, &[], 0.8705),
    ("Crown Main 1", 35.0, &[0.0, 48.0], 0.5982),
    ("Crown Main 2", 35.0, &[12.0, 36.0, 60.0, 84.0], 0.706),
    ("Crown Main 3", 35.0, &[24.0, 72.0], 0.7989),
    ("Star 1", 18.0, &[6.0, 42.0, 54.0, 90.0], 0.4812),
    ("Star 2", 18.0, &[18.0, 30.0, 66.0, 78.0], 0.5797),
    ("Upper Girdle 1", 41.0, &[1.0, 47.0, 49.0, 95.0], 0.6423),
    ("Upper Girdle 2", 41.0, &[11.0, 37.0, 59.0, 85.0], 0.7528),
    ("Upper Girdle 3", 41.0, &[13.0, 35.0, 61.0, 83.0], 0.7953),
    ("Upper Girdle 4", 41.0, &[23.0, 25.0, 71.0, 73.0], 0.8814),
    ("Table", 0.0, &[], 0.34),
];

/// Cushion: a rounded square (girdle masts `0.5 (|cos a| + |sin a|) + 0.5`), 4-fold
/// with mirror, crown 38 / pavilion 43 degrees, the quartz-like figures. Stored in
/// cutting order: the pavilion mains, the girdle, the lower girdle and the culet, then the
/// crown mains, the star and the upper girdle, the table last.
pub(super) const CUSHION_ROWS: &[Row] = &[
    ("Pavilion Main 1", -43.0, &[0.0, 24.0, 48.0, 72.0], 0.7039),
    ("Pavilion Main 2", -43.0, &[12.0, 36.0, 60.0, 84.0], 0.8452),
    ("Girdle 1", 90.0, &[0.0, 24.0, 48.0, 72.0], 1.0),
    (
        "Girdle 2",
        90.0,
        &[6.0, 18.0, 30.0, 42.0, 54.0, 66.0, 78.0, 90.0],
        1.1533,
    ),
    ("Girdle 3", 90.0, &[12.0, 36.0, 60.0, 84.0], 1.2071),
    (
        "Lower Girdle 1",
        -44.0,
        &[1.0, 23.0, 25.0, 47.0, 49.0, 71.0, 73.0, 95.0],
        0.7148,
    ),
    (
        "Lower Girdle 2",
        -44.0,
        &[11.0, 13.0, 35.0, 37.0, 59.0, 61.0, 83.0, 85.0],
        0.8583,
    ),
    ("Culet", -0.0, &[], 0.9357),
    ("Crown Main 1", 38.0, &[0.0, 24.0, 48.0, 72.0], 0.6393),
    ("Crown Main 2", 38.0, &[12.0, 36.0, 60.0, 84.0], 0.7668),
    (
        "Star",
        20.0,
        &[6.0, 18.0, 30.0, 42.0, 54.0, 66.0, 78.0, 90.0],
        0.5165,
    ),
    (
        "Upper Girdle 1",
        42.0,
        &[1.0, 23.0, 25.0, 47.0, 49.0, 71.0, 73.0, 95.0],
        0.7,
    ),
    (
        "Upper Girdle 2",
        42.0,
        &[11.0, 13.0, 35.0, 37.0, 59.0, 61.0, 83.0, 85.0],
        0.8283,
    ),
    ("Table", 0.0, &[], 0.33),
];

/// Emerald step cut, length-to-width 1.4, 2-fold with mirror: three crown steps
/// (34 / 38 / 42 degrees towards the girdle) and three pavilion steps (42 / 45 / 48
/// degrees towards the girdle) on the long sides, the short ends and the cut
/// corners, meeting in a keel. Beryl-like figures.
pub(super) const EMERALD_ROWS: &[Row] = &[
    ("Table", 0.0, &[], 0.3292),
    ("Crown Side 1", 42.0, &[0.0, 48.0], 0.6914),
    ("Crown Side 2", 38.0, &[0.0, 48.0], 0.6512),
    ("Crown Side 3", 34.0, &[0.0, 48.0], 0.6191),
    ("Crown End 1", 42.0, &[24.0, 72.0], 0.9591),
    ("Crown End 2", 38.0, &[24.0, 72.0], 0.8975),
    ("Crown End 3", 34.0, &[24.0, 72.0], 0.8428),
    ("Crown Corner 1", 42.0, &[12.0, 36.0, 60.0, 84.0], 0.997),
    ("Crown Corner 2", 38.0, &[12.0, 36.0, 60.0, 84.0], 0.9324),
    ("Crown Corner 3", 34.0, &[12.0, 36.0, 60.0, 84.0], 0.8744),
    ("Girdle Side", 90.0, &[0.0, 48.0], 1.0),
    ("Girdle End", 90.0, &[24.0, 72.0], 1.4),
    ("Girdle Corner", 90.0, &[12.0, 36.0, 60.0, 84.0], 1.4566),
    ("Pavilion Side 1", -48.0, &[0.0, 48.0], 0.7632),
    ("Pavilion Side 2", -45.0, &[0.0, 48.0], 0.7518),
    ("Pavilion Side 3", -42.0, &[0.0, 48.0], 0.7627),
    ("Pavilion End 1", -48.0, &[24.0, 72.0], 1.0605),
    ("Pavilion End 2", -45.0, &[24.0, 72.0], 1.0346),
    ("Pavilion End 3", -42.0, &[24.0, 72.0], 1.0304),
    (
        "Pavilion Corner 1",
        -48.0,
        &[12.0, 36.0, 60.0, 84.0],
        1.1026,
    ),
    (
        "Pavilion Corner 2",
        -45.0,
        &[12.0, 36.0, 60.0, 84.0],
        1.0535,
    ),
    (
        "Pavilion Corner 3",
        -42.0,
        &[12.0, 36.0, 60.0, 84.0],
        1.0261,
    ),
];

/// Princess: a square with clipped corners (girdle masts 1.0 on the sides, 1.3 on
/// the diagonals), 4-fold with mirror, crown 35 / pavilion 41 degrees, the
/// corundum-like figures. A modified brilliant: the pavilion has side and corner
/// mains, not chevrons. Stored in cutting order: the pavilion mains, the girdle, the lower
/// girdle and the culet, then the crown mains, the star and the upper girdle, the table
/// last.
pub(super) const PRINCESS_ROWS: &[Row] = &[
    ("Pavilion Main 1", -41.0, &[0.0, 24.0, 48.0, 72.0], 0.6787),
    ("Pavilion Main 2", -41.0, &[12.0, 36.0, 60.0, 84.0], 0.8755),
    ("Girdle 1", 90.0, &[0.0, 24.0, 48.0, 72.0], 1.0),
    ("Girdle 2", 90.0, &[12.0, 36.0, 60.0, 84.0], 1.3),
    (
        "Lower Girdle 1",
        -42.0,
        &[1.0, 23.0, 25.0, 47.0, 49.0, 71.0, 73.0, 95.0],
        0.7225,
    ),
    (
        "Lower Girdle 2",
        -42.0,
        &[11.0, 13.0, 35.0, 37.0, 59.0, 61.0, 83.0, 85.0],
        0.8878,
    ),
    ("Culet", -0.0, &[], 0.8705),
    ("Crown Main 1", 35.0, &[0.0, 24.0, 48.0, 72.0], 0.5982),
    ("Crown Main 2", 35.0, &[12.0, 36.0, 60.0, 84.0], 0.7702),
    (
        "Star",
        18.0,
        &[6.0, 18.0, 30.0, 42.0, 54.0, 66.0, 78.0, 90.0],
        0.4927,
    ),
    (
        "Upper Girdle 1",
        41.0,
        &[1.0, 23.0, 25.0, 47.0, 49.0, 71.0, 73.0, 95.0],
        0.7048,
    ),
    (
        "Upper Girdle 2",
        41.0,
        &[11.0, 13.0, 35.0, 37.0, 59.0, 61.0, 83.0, 85.0],
        0.8537,
    ),
    ("Table", 0.0, &[], 0.33),
];
