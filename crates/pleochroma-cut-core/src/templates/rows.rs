use super::Row;

// The four non-round shapes, each a meet-point design (`crate::design::Design::cut_meets`),
// stored in the order it is cut: the girdle outline first, then the pavilion and the crown, and
// the table last.
// The masts are full precision: each tier must pass through the point its comment names on the
// stone cut so far, and the girdle line must be level (`crate::design::Design::girdle_level`). A
// rounded mast floats off its point. On the default preform, and after a remap to each gear the
// template offers, every facet reaches the surface, none is too small to cut, no preform plane
// cuts the stone and the outline keeps its length-to-width ratio. The meets survive a remap only
// where every index lands on a whole tooth (the emerald on every offered gear). The tests in
// `tests.rs` and `tests/meets.rs` report what breaks when a mast is edited by hand.

/// Oval, length-to-width 1.35, 2-fold with mirror: a meet-point design, stored in the order it is
/// cut. Girdle masts follow the ellipse `hypot(cos a, 1.35 sin a)` around the 16 girdle positions;
/// corundum-like figures.
///
/// - The girdle outline comes first: Girdle 1 sets the stone size, the other four are cut to depth
///   before any facet leaves a point to meet.
/// - Every pavilion tier meets a point on the girdle that its own facets turn about when a
///   retarget changes the angles, so no pavilion meet rests on an angle relation:
///   - the diagonal mains set the girdle line on Girdle 3;
///   - Lower Girdle 1 meets their corner with Girdle 2 and is level on Girdle 2; Lower Girdle 2
///     meets the corner Lower Girdle 1 leaves on Girdle 1 and dips into a V in the middle of
///     Girdle 1, and the side main meets that V;
///   - Lower Girdle 3 and 4 and the end main do the same on Girdle 4 and Girdle 5.
/// - The side mains meet in a short keel along the length; the diagonal mains close its ends and
///   the end mains end on the diagonal mains' ridge beyond it: points of three planes, or of two
///   mirror pairs. Mains 41 degrees on the diagonals, 40.5 on the sides, 41.5 on the ends; lower
///   girdles 43 to 45.
/// - The crown: the diagonal crown mains set the girdle width and meet in the crown's point; the
///   side and end mains meet that point at the angles that put each level on its own girdle face
///   (40.28 and 32.12 degrees). Each upper girdle meets its main's girdle corner, and the
///   second of each pair closes the pair on the ridge between two mains at its exact, unrounded
///   angle. Each star meets its four-facet star point, and the table the side and end kite tops,
///   which Star 2's exact angle puts level. A retarget scales every crown tangent alike, which
///   keeps these points.
/// - The upper girdles straddle Girdle 2 and Girdle 4 one tooth either side (5/7, 17/19), so the
///   peak each pair raises on the girdle there is low.
///
/// The masts are full precision: a rounded mast floats off its point, and a rounded closing angle
/// misses its point by more than a retarget can hold. The girdle line is 2.75 % of the width thick
/// at every girdle corner, crown and pavilion alike.
pub(super) const OVAL_ROWS: &[Row] = &[
    // Set stone size.
    ("Girdle 1", 90.0, &[0.0, 48.0], 1.0),
    // Nothing to meet yet: cut to depth with the outline.
    ("Girdle 2", 90.0, &[6.0, 42.0, 54.0, 90.0], 1.0585),
    // Nothing to meet yet: cut to depth with the outline.
    ("Girdle 3", 90.0, &[12.0, 36.0, 60.0, 84.0], 1.188),
    // Nothing to meet yet: cut to depth with the outline.
    ("Girdle 4", 90.0, &[18.0, 30.0, 66.0, 78.0], 1.3046),
    // Nothing to meet yet: cut to depth with the outline.
    ("Girdle 5", 90.0, &[24.0, 72.0], 1.35),
    // Set girdle line: level at the girdle line on Girdle 3.
    (
        "Pavilion Main 1",
        -41.0,
        &[12.0, 36.0, 60.0, 84.0],
        0.800_152_634_588_976_6,
    ),
    // Meets Girdle 2, Girdle 3 and Pavilion Main 1 at their corner; level on Girdle 2.
    (
        "Lower Girdle 1",
        -43.5,
        &[6.0, 42.0, 54.0, 90.0],
        0.748_571_276_219_376_6,
    ),
    // Meets Girdle 1, Girdle 2 and Lower Girdle 1 at their corner; each pair dips into a V
    // in the middle of Girdle 1.
    (
        "Lower Girdle 2",
        -43.0,
        &[1.0, 47.0, 49.0, 95.0],
        0.716_341_551_255_049,
    ),
    // Meets Girdle 1 and Lower Girdle 2 at that V (1e-7 short of it, so it touches the
    // girdle face in a point); the two side mains meet in the keel.
    (
        "Pavilion Main 2",
        -40.5,
        &[0.0, 48.0],
        0.686_673_595_938_459_9,
    ),
    // Meets Girdle 3, Girdle 4 and Pavilion Main 1 at their corner; level on Girdle 4.
    (
        "Lower Girdle 3",
        -45.0,
        &[18.0, 30.0, 66.0, 78.0],
        0.941_937_105_066_988_9,
    ),
    // Meets Girdle 4, Girdle 5 and Lower Girdle 3 at their corner; each pair dips into a V
    // in the middle of Girdle 5.
    (
        "Lower Girdle 4",
        -44.0,
        &[23.0, 25.0, 71.0, 73.0],
        0.962_373_104_739_941_2,
    ),
    // Meets Girdle 5 and Lower Girdle 4 at that V (1e-7 short of it); it ends on the
    // ridge of the diagonal mains beyond the keel.
    (
        "Pavilion Main 3",
        -41.5,
        &[24.0, 72.0],
        0.922_223_928_467_858_1,
    ),
    // Set girdle width: level at the girdle line on Girdle 3, meeting in the crown's point.
    (
        "Crown Main 1",
        35.5,
        &[12.0, 36.0, 60.0, 84.0],
        0.712_263_276_242_024_9,
    ),
    // Meets Crown Main 1 at the crown's point; level on Girdle 1.
    ("Crown Main 2", 40.28, &[0.0, 48.0], 0.667_450_058_534_227_2),
    // Meets Crown Main 1 and 2 at the crown's point; level on Girdle 5.
    (
        "Crown Main 3",
        32.12,
        &[24.0, 72.0],
        0.740_978_016_933_396_6,
    ),
    // Meets Girdle 1, Girdle 2 and Crown Main 2 at their corner.
    (
        "Upper Girdle 1",
        46.05,
        &[5.0, 43.0, 53.0, 91.0],
        0.782_184_609_279_504_4,
    ),
    // Meets Girdle 2, Girdle 3 and Crown Main 1 at their corner, and the point
    // Upper Girdle 1 left on the ridge of Crown Main 1 and 2 (exact angle).
    (
        "Upper Girdle 2",
        44.938_652_484_073,
        &[7.0, 41.0, 55.0, 89.0],
        0.790_896_218_193_703_8,
    ),
    // Meets Girdle 3, Girdle 4 and Crown Main 1 at their corner.
    (
        "Upper Girdle 3",
        35.83,
        &[17.0, 31.0, 65.0, 79.0],
        0.782_620_445_775_155_8,
    ),
    // Meets Girdle 4, Girdle 5 and Crown Main 3 at their corner, and the point
    // Upper Girdle 3 left on the ridge of Crown Main 1 and 3 (exact angle).
    (
        "Upper Girdle 4",
        35.208_670_959_117,
        &[19.0, 29.0, 67.0, 77.0],
        0.787_196_200_568_134_1,
    ),
    // Meets Crown Main 1 and 2 and Upper Girdle 1 and 2 at the star point.
    (
        "Star 1",
        16.89,
        &[6.0, 42.0, 54.0, 90.0],
        0.511_675_204_645_489_4,
    ),
    // Meets Crown Main 1 and 3 and Upper Girdle 3 and 4 at the star point; its exact angle
    // puts the end kite top level with the side kite top.
    (
        "Star 2",
        20.420_213_935_725,
        &[18.0, 30.0, 66.0, 78.0],
        0.604_482_177_995_067_6,
    ),
    // Meets Crown Main 2 and 3 and the stars at the side and end kite tops.
    ("Table", 0.0, &[], 0.366_436_750_224_219_7),
];

/// Cushion: a rounded square (girdle masts `0.5 (|cos a| + |sin a|) + 0.5`), 4-fold
/// with mirror, the quartz-like figures: pavilion mains 42 (sides) and 41.25 (corners)
/// degrees, crown mains 38 and 33.5 degrees. A meet-point design, stored in cutting order:
/// the girdle first, then the pavilion and the crown, the table last.
///
/// - The girdle is ground first; its first tier sets the stone size.
/// - The first breaks set the girdle line (pavilion) or the girdle width (crown) through the
///   corners where the side face meets the rounding faces. The side mains meet the V
///   (pavilion) or the peak (crown) those breaks leave in the middle of the side face.
/// - The second breaks are cut at the rounding faces' own index: each meets the corner of the
///   side face and runs level along its rounding face. The corner mains meet the next corner,
///   where the rounding face meets the corner face, and run level along the corner face. The
///   girdle line is carried round the corner by facets parallel to the girdle faces, not by
///   an angle relation, so it is level (2.75 % of the width at every girdle-face corner)
///   whatever the angles, and a retarget keeps it level.
/// - Every point a tier meets is a corner of the finished stone where three planes meet at
///   clear angles: the stars meet the star point (the upper girdle and the two crown mains),
///   the table the tops of the side kites. The retarget's projection holds those corners, so
///   no tier needs re-anchoring after it.
///
/// The masts are full-precision values worked out that way; a rounded mast floats off its
/// point.
pub(super) const CUSHION_ROWS: &[Row] = &[
    // Set stone size.
    ("Girdle 1", 90.0, &[0.0, 24.0, 48.0, 72.0], 1.0),
    // Nothing to meet yet: ground to the outline with Girdle 1.
    (
        "Girdle 2",
        90.0,
        &[6.0, 18.0, 30.0, 42.0, 54.0, 66.0, 78.0, 90.0],
        1.1533,
    ),
    // Nothing to meet yet: ground to the outline with Girdle 1.
    ("Girdle 3", 90.0, &[12.0, 36.0, 60.0, 84.0], 1.2071),
    // Set girdle line: through the corners where the side face (Girdle 1) meets the
    // rounding faces (Girdle 2).
    (
        "Lower Girdle 1",
        -44.0,
        &[1.0, 23.0, 25.0, 47.0, 49.0, 71.0, 73.0, 95.0],
        0.740_190_120_906_136_7,
    ),
    // Meets Girdle 1 and Lower Girdle 1: the point of the lower girdles' V in the middle
    // of the side face. It stops 1e-7 short of the point, well inside the meet
    // tolerance, so it touches the girdle face in a point and never runs along it.
    (
        "Pavilion Main 1",
        -42.0,
        &[0.0, 24.0, 48.0, 72.0],
        0.717_705_783_959_161_8,
    ),
    // Meets Girdle 1, Girdle 2 and Lower Girdle 1 at the corner where the side face meets
    // the rounding face. Cut at the rounding face's own index, it runs level along that
    // face to its corner with the corner face: the girdle line, whatever the angle.
    (
        "Lower Girdle 2",
        -41.5,
        &[6.0, 18.0, 30.0, 42.0, 54.0, 66.0, 78.0, 90.0],
        0.784_796_091_006_928_3,
    ),
    // Meets Girdle 2, Girdle 3 and Lower Girdle 2 at the corner where the rounding face
    // meets the corner face, and runs level along the corner face.
    (
        "Pavilion Main 2",
        -41.25,
        &[12.0, 36.0, 60.0, 84.0],
        0.816_572_038_277_176_3,
    ),
    // Set girdle width: through the corners where the side face meets the rounding faces.
    (
        "Upper Girdle 1",
        42.0,
        &[1.0, 23.0, 25.0, 47.0, 49.0, 71.0, 73.0, 95.0],
        0.714_370_710_225_311_8,
    ),
    // Meets Girdle 1 and Upper Girdle 1: the peak of the upper girdles in the middle of
    // the side face (the kite's lower point). It stops 1e-7 short of the peak, as
    // Pavilion Main 1 does.
    (
        "Crown Main 1",
        38.0,
        &[0.0, 24.0, 48.0, 72.0],
        0.665_152_142_986_851_5,
    ),
    // Meets Girdle 1, Girdle 2 and Upper Girdle 1 at the corner where the side face meets
    // the rounding face, and runs level along the rounding face, as Lower Girdle 2 does.
    (
        "Upper Girdle 2",
        42.0,
        &[6.0, 18.0, 30.0, 42.0, 54.0, 66.0, 78.0, 90.0],
        0.792_144_873_682_160_1,
    ),
    // Meets Girdle 2, Girdle 3 and Upper Girdle 2 at the corner where the rounding face
    // meets the corner face, and runs level along the corner face.
    (
        "Crown Main 2",
        33.5,
        &[12.0, 36.0, 60.0, 84.0],
        0.689_174_977_420_881_1,
    ),
    // Meets Upper Girdle 1, Crown Main 1 and Crown Main 2 where the two mains and the upper
    // girdle meet: the star's point.
    (
        "Star",
        18.0,
        &[6.0, 18.0, 30.0, 42.0, 54.0, 66.0, 78.0, 90.0],
        0.509_803_191_569_409_7,
    ),
    // Meets Crown Main 1 and Star at the top of each side kite.
    ("Table", 0.0, &[], 0.343_826_932_382_329_3),
];

/// Emerald step cut, length-to-width 1.4, 2-fold with mirror: three crown steps and three
/// pavilion steps on the long sides, the short ends and the cut corners, the pavilion meeting in
/// a keel. Beryl-like figures. A meet-point design ([`crate::design::Design::cut_meets`]),
/// stored in cutting order: the girdle, then the pavilion and the crown, each led by its end
/// steps.
///
/// - The first end step sets the girdle line (pavilion) or the girdle width (crown); the first
///   corner and side steps carry that line round the corner, so the girdle is level.
/// - The third end step goes in before the second: it meets the point where the first crosses
///   the edge between the girdle side and the girdle corner.
/// - The second end step is then cut between the two, through the point where the first corner
///   and side steps end on the third end step; the other second and third steps meet the
///   corners the steps before them left.
/// - The table meets the tops of the third crown corner steps, so it is a rectangle; a table cut
///   lower has no point to meet.
///
/// The crown end steps are shallower than the crown side steps, so the crown steps are about as
/// wide on the ends as on the sides. The masts are full-precision values worked out that way; a
/// rounded mast floats off its point.
pub(super) const EMERALD_ROWS: &[Row] = &[
    // Set stone size.
    ("Girdle Side", 90.0, &[0.0, 48.0], 1.0),
    // Nothing to meet yet: the girdle faces form no point of their own.
    ("Girdle End", 90.0, &[24.0, 72.0], 1.4),
    // Nothing to meet yet: the girdle faces form no point of their own.
    ("Girdle Corner", 90.0, &[12.0, 36.0, 60.0, 84.0], 1.4566),
    // Set girdle line: the first pavilion step, on the girdle line of the end face.
    (
        "Pavilion End 1",
        -45.0,
        &[24.0, 72.0],
        1.009_394_930_143_796_6,
    ),
    // Meets Girdle Side, Girdle Corner and Pavilion End 1, where End 1 crosses the edge between
    // the side and the corner girdle faces.
    (
        "Pavilion End 3",
        -42.0,
        &[24.0, 72.0],
        0.982_388_349_760_572_1,
    ),
    // Meets Girdle End, Girdle Corner and Pavilion End 1 on the girdle line.
    (
        "Pavilion Corner 1",
        -45.0,
        &[12.0, 36.0, 60.0, 84.0],
        1.049_417_173_958_955_3,
    ),
    // Meets Girdle Side, Girdle Corner and Pavilion Corner 1 on the girdle line.
    (
        "Pavilion Side 1",
        -45.0,
        &[0.0, 48.0],
        0.726_552_217_669_177_7,
    ),
    // Meets Pavilion End 3, Pavilion Corner 1 and Pavilion Side 1, where the first corner and side
    // steps end on End 3. It is cut between End 1 and End 3.
    (
        "Pavilion End 2",
        -43.0,
        &[24.0, 72.0],
        0.986_418_059_734_894_4,
    ),
    // Meets Pavilion End 1, Pavilion Corner 1 and Pavilion End 2.
    (
        "Pavilion Corner 2",
        -43.0,
        &[12.0, 36.0, 60.0, 84.0],
        1.025_019_166_914_432,
    ),
    // Meets Pavilion End 3, Pavilion End 2 and Pavilion Corner 2.
    (
        "Pavilion Side 3",
        -41.0,
        &[0.0, 48.0],
        0.709_555_989_861_880_8,
    ),
    // Meets Pavilion Side 1, Pavilion Corner 2 and Pavilion Side 3.
    (
        "Pavilion Corner 3",
        -41.0,
        &[12.0, 36.0, 60.0, 84.0],
        1.004_424_812_616_545,
    ),
    // Meets Pavilion Corner 1, Pavilion Side 1 and Pavilion Corner 2.
    (
        "Pavilion Side 2",
        -43.0,
        &[0.0, 48.0],
        0.713_618_715_709_895_1,
    ),
    // Set girdle width: the first crown step, on the girdle line of the end face.
    ("Crown End 1", 33.0, &[24.0, 72.0], 0.785_558_089_639_537_1),
    // Meets Girdle Side, Girdle Corner and Crown End 1, where End 1 crosses the edge between the
    // side and the corner girdle faces.
    ("Crown End 3", 23.0, &[24.0, 72.0], 0.642_746_749_354_888_8),
    // Meets Girdle End, Girdle Corner and Crown End 1 on the girdle line.
    (
        "Crown Corner 1",
        43.0,
        &[12.0, 36.0, 60.0, 84.0],
        1.013_511_038_061_562_6,
    ),
    // Meets Girdle Side, Girdle Corner and Crown Corner 1 on the girdle line.
    ("Crown Side 1", 42.0, &[0.0, 48.0], 0.689_567_089_059_486_8),
    // Meets Crown End 3, Crown Corner 1 and Crown Side 1, where the first corner and side steps
    // end on End 3. It is cut between End 1 and End 3.
    ("Crown End 2", 29.0, &[24.0, 72.0], 0.718_770_461_164_273_1),
    // Meets Crown End 1, Crown Corner 1 and Crown End 2.
    (
        "Crown Corner 2",
        41.0,
        &[12.0, 36.0, 60.0, 84.0],
        0.982_756_719_625_83,
    ),
    // Meets Crown End 3, Crown End 2 and Crown Corner 2.
    ("Crown Side 3", 34.0, &[0.0, 48.0], 0.628_726_243_684_782_9),
    // Meets Crown Side 1, Crown Corner 2 and Crown Side 3.
    (
        "Crown Corner 3",
        27.0,
        &[12.0, 36.0, 60.0, 84.0],
        0.773_061_590_876_325_2,
    ),
    // Meets Crown Corner 1, Crown Side 1 and Crown Corner 2.
    ("Crown Side 2", 40.0, &[0.0, 48.0], 0.670_363_274_756_369_5),
    // Meets Crown End 3, Crown Side 3 and Crown Corner 3: the top of each third corner step, so
    // the table is a rectangle.
    ("Table", 0.0, &[], 0.339_824_937_735_390_6),
];

/// Princess: a square with clipped corners (girdle masts 1.0 on the sides, 1.3 on
/// the diagonals), 4-fold with mirror, crown 35 / pavilion 41 degrees, the
/// corundum-like figures. A modified brilliant: side and corner mains on both halves,
/// a pair of break facets at each corner, stars and a table. A meet-point design, stored
/// in cutting order: the girdle first, then the lower girdle (it sets the girdle line at
/// the girdle corners) and the pavilion mains, which meet it; the four side mains end in
/// the culet point. The crown repeats it: the upper girdle sets the girdle width, the
/// crown mains meet it, the stars meet the points above the girdle corners, and the table
/// meets the tops of the side mains. The girdle line is level, 2.75 % of the width thick,
/// at every corner. The masts are full-precision meet values; a rounded mast floats off
/// its point.
pub(super) const PRINCESS_ROWS: &[Row] = &[
    // Set stone size: the four sides, 1.0 from the axis.
    ("Girdle 1", 90.0, &[0.0, 24.0, 48.0, 72.0], 1.0),
    // Nothing to meet yet: cut the corners to depth, 1.3 from the axis.
    ("Girdle 2", 90.0, &[12.0, 36.0, 60.0, 84.0], 1.3),
    // Set girdle line: each facet runs through a corner where a side meets a corner face, at
    // -0.0275 (the line is 2.75 % of the width thick), and the pair either side of a corner meets
    // in a shallow V on the corner face.
    (
        "Lower Girdle",
        -42.0,
        &[11.0, 13.0, 35.0, 37.0, 59.0, 61.0, 83.0, 85.0],
        0.893_442_124_376_700_5,
    ),
    // Meets Girdle 1, Girdle 2 and Lower Girdle at the girdle corners, so the bottom of the girdle
    // is level along each side; the four end in one point, the culet.
    (
        "Pavilion Main 1",
        -41.0,
        &[0.0, 24.0, 48.0, 72.0],
        0.676_813_600_196_478_3,
    ),
    // Meets Girdle 2 and Lower Girdle at the point of each corner V.
    (
        "Pavilion Main 2",
        -41.0,
        &[12.0, 36.0, 60.0, 84.0],
        0.878_707_341_033_315,
    ),
    // Set girdle width: each facet runs through a girdle corner at +0.0275, the pair either side
    // of a corner meets in a shallow V on the corner face.
    (
        "Upper Girdle",
        39.0,
        &[11.0, 13.0, 35.0, 37.0, 59.0, 61.0, 83.0, 85.0],
        0.842_437_274_723_097,
    ),
    // Meets Girdle 1, Girdle 2 and Upper Girdle at the girdle corners, so the top of the girdle is
    // level along each side.
    (
        "Crown Main 1",
        35.0,
        &[0.0, 24.0, 48.0, 72.0],
        0.596_103_154_282_790_6,
    ),
    // Meets Girdle 2 and Upper Girdle at the point of each corner V.
    (
        "Crown Main 2",
        37.0,
        &[12.0, 36.0, 60.0, 84.0],
        0.809_152_853_887_968_5,
    ),
    // Meets Upper Girdle, Crown Main 1 and Crown Main 2 above each girdle corner: the star point.
    (
        "Star",
        22.0,
        &[6.0, 18.0, 30.0, 42.0, 54.0, 66.0, 78.0, 90.0],
        0.513_913_869_172_092_9,
    ),
    // Meets Crown Main 1 and Star at the top of each side main.
    ("Table", 0.0, &[], 0.356_260_217_568_301_73),
];
