use super::*;

/// Downtown Austin, the airport and a point far outside every encoded area.
const DOWNTOWN: (f64, f64) = (30.267, -97.743);
const AIRPORT: (f64, f64) = (30.197, -97.666);
const FAR_AWAY: (f64, f64) = (40.0, -100.0);

fn coords(encoded: Encoded) -> Vec<u128> {
    match encoded {
        Encoded::Inside(c) | Encoded::Clamped(c) => c,
        Encoded::Dropped => panic!("location was dropped"),
    }
}

/// Every coordinate of an encoded location fits in `axis_bits` bits.
fn assert_fits(encoder: &dyn LocationEncoder, c: &[u128]) {
    assert_eq!(c.len(), encoder.dims());
    assert!(c.iter().all(|&v| v < 1u128 << encoder.axis_bits()), "{:?}", c);
}

/// Encoding the center of a cell gives the same cell back.
fn assert_round_trip(encoder: &dyn LocationEncoder, (lat, lon): (f64, f64)) {
    let c = coords(encoder.encode(lat, lon));
    assert_fits(encoder, &c);
    let (clat, clon) = encoder.decode(&c).expect("an encoded cell decodes");
    assert_eq!(coords(encoder.encode(clat, clon)), c, "center of {c:?}");
}

#[test]
fn mosaic_grid_matches_geo_to_grid() {
    let grid = MosaicGrid;
    // Downtown is exactly one degree from the origin on both axes.
    assert_eq!(
        grid.encode(DOWNTOWN.0, DOWNTOWN.1),
        Encoded::Inside(vec![1000, 1000])
    );
    for (lat, lon) in [DOWNTOWN, AIRPORT, (30.4012, -97.7254)] {
        let (lat_grid, lon_grid) = crate::data_toolkit::geo_to_grid(lat, lon);
        assert_eq!(
            coords(grid.encode(lat, lon)),
            vec![lat_grid as u128, lon_grid as u128]
        );
        assert_round_trip(&grid, (lat, lon));
    }
}

#[test]
fn mosaic_grid_clamps_outside_points() {
    let grid = MosaicGrid;
    assert_eq!(
        grid.encode(FAR_AWAY.0, FAR_AWAY.1),
        Encoded::Clamped(vec![1999, 0])
    );
    assert_eq!(grid.encode(28.0, -97.743), Encoded::Clamped(vec![0, 1000]));
    assert!(grid.decode(&[2000, 0]).is_none());
}

#[test]
fn metadata_path_sits_next_to_the_points() {
    assert_eq!(
        metadata_path("data/h3/start_busiest_day.json"),
        PathBuf::from("data/h3/start_busiest_day.encoding.json")
    );
}

#[test]
fn metadata_round_trips_through_json() {
    let metadata = EncodingMetadata {
        encoding: Encoding::MosaicGrid,
        point_kind: PointKind::Start,
        d: 2,
        h1: 11,
        total_rides: 3,
        kept_rides: 2,
        clamped_rides: 1,
        dropped_rides: 1,
    };
    let json = serde_json::to_string(&metadata).unwrap();
    assert_eq!(serde_json::from_str::<EncodingMetadata>(&json).unwrap(), metadata);
}

fn default_morton() -> Morton {
    Morton::new(29.9005, 30.9245, -98.0505, -97.0265, 10)
}

#[test]
fn default_morton_cells_are_mosaic_grid_cells() {
    let (morton, grid) = (default_morton(), MosaicGrid);
    // Every 3-decimal coordinate in the box (RideAustin's precision) lands in the Mosaic grid
    // cell shifted by (634, 693).
    for lat_milli in (29_901..=30_924).step_by(7) {
        for lon_milli in (-98_050..=-97_027).step_by(11) {
            let (lat, lon) = (lat_milli as f64 / 1000.0, lon_milli as f64 / 1000.0);
            let m = coords(morton.encode(lat, lon));
            let g = coords(grid.encode(lat, lon));
            assert_eq!(m, vec![g[0] - 634, g[1] - 693], "({}, {})", lat, lon);
        }
    }
}

#[test]
fn morton_drops_outside_points_and_round_trips() {
    let morton = default_morton();
    for p in [DOWNTOWN, AIRPORT] {
        assert_round_trip(&morton, p);
    }
    assert_eq!(morton.encode(FAR_AWAY.0, FAR_AWAY.1), Encoded::Dropped);
    assert_eq!(morton.encode(29.9004, -97.743), Encoded::Dropped);
    assert_eq!(morton.encode(30.9245, -97.743), Encoded::Dropped);
    assert_eq!(morton.encode(f64::NAN, -97.743), Encoded::Dropped);
    assert_eq!(morton.encode(29.9005, -98.0505), Encoded::Inside(vec![0, 0]));
    assert!(morton.decode(&[1024, 0]).is_none());
}

#[test]
fn morton_encoding_parses_from_metadata_json() {
    let json = r#"{"type":"morton","min_lat":29.9005,"max_lat":30.9245,"min_lon":-98.0505,"max_lon":-97.0265,"axis_bits":10}"#;
    let encoding: Encoding = serde_json::from_str(json).unwrap();
    assert_eq!(encoding.encoder().axis_bits(), 10);
    assert_eq!(encoding.name(), "morton");
}

#[test]
fn geohash_matches_reference_geohashes() {
    // The Wikipedia example, and Austin cells checked with exact rational arithmetic (the airport
    // is the cell in Mastic's syn-geohash.log).
    let base32 = |lat, lon, n| geohash::bits_to_base32(&geohash::point_to_bits(lat, lon, n));
    assert_eq!(base32(57.64911, 10.40744, 55), "u4pruydqqvj");
    assert_eq!(base32(DOWNTOWN.0, DOWNTOWN.1, 35), "9v6kpvc");
    assert_eq!(base32(AIRPORT.0, AIRPORT.1, 35), "9v6ec91");
    assert_eq!(geohash::bits_to_base32(&geohash::base32_to_bits("9v6")), "9v6");
}

#[test]
fn geohash_splits_the_bits_after_the_prefix_into_lat_and_lon() {
    let gh = Geohash::new("9v6", 21);
    assert_eq!((gh.dims(), gh.axis_bits()), (2, 11));
    for p in [DOWNTOWN, AIRPORT, (30.4012, -97.7254)] {
        assert_round_trip(&gh, p);
        let c = coords(gh.encode(p.0, p.1));
        assert!(c[1] < 1 << 10, "longitude has 10 bits: {:?}", c);
        // The label is the standard 36-bit geohash of the point (7 characters and a bit).
        let full = geohash::point_to_bits(p.0, p.1, 36);
        assert_eq!(gh.label(&c), geohash::bits_to_base32(&full));
    }
    // Neighbouring cells differ by one in one coordinate: 0.000687 degrees north, 0.001373 east.
    let c = coords(gh.encode(DOWNTOWN.0, DOWNTOWN.1));
    let (lat, lon) = gh.decode(&c).unwrap();
    assert_eq!(coords(gh.encode(lat + 0.000687, lon)), vec![c[0] + 1, c[1]]);
    assert_eq!(coords(gh.encode(lat, lon + 0.001373)), vec![c[0], c[1] + 1]);
    assert_eq!(gh.encode(FAR_AWAY.0, FAR_AWAY.1), Encoded::Dropped);
    assert!(gh.decode(&[0, 1 << 10]).is_none());
}

fn default_h3() -> H3 {
    H3::new(10, DOWNTOWN.0, DOWNTOWN.1, 11)
}

/// Resolution 10 cells around a point, out to `k` steps.
fn h3_disk(h3: &H3, (lat, lon): (f64, f64), k: u32) -> Vec<(h3o::CellIndex, Vec<u128>)> {
    let center = h3.cell(lat, lon).unwrap();
    center
        .grid_disk::<Vec<_>>(k)
        .into_iter()
        .map(|cell| {
            let c = h3o::LatLng::from(cell);
            (cell, coords(h3.encode(c.lat(), c.lng())))
        })
        .collect()
}

fn diff(a: u128, b: u128) -> i64 {
    a as i64 - b as i64
}

#[test]
fn h3_linf_is_the_grid_distance() {
    let h3 = default_h3();
    for place in [DOWNTOWN, AIRPORT] {
        let disk = h3_disk(&h3, place, 12);
        let (center, c0) = &disk[0];
        for (cell, c) in &disk {
            let linf = (0..3).map(|k| diff(c[k], c0[k]).abs()).max().unwrap();
            assert_eq!(linf, center.grid_distance(*cell).unwrap() as i64, "{}", cell);
        }
    }
}

#[test]
fn h3_l2_is_proportional_to_the_center_distance() {
    let h3 = default_h3();
    let disk = h3_disk(&h3, DOWNTOWN, 8);
    let (center, c0) = &disk[0];
    let center_ll = h3o::LatLng::from(*center);
    let neighbour = h3o::LatLng::from(disk[1].0);
    let spacing = center_ll.distance_m(neighbour);
    for (cell, c) in &disk[1..] {
        let l2_squared: i64 = (0..3).map(|k| diff(c[k], c0[k]).pow(2)).sum();
        let center_distance = center_ll.distance_m(h3o::LatLng::from(*cell)) / spacing;
        let ratio = (l2_squared as f64 / 2.0).sqrt() / center_distance;
        // H3 hexagons are slightly distorted, so the ratio is close to, not exactly, 1.
        assert!((ratio - 1.0).abs() < 0.02, "{}: ratio {}", cell, ratio);
    }
}

#[test]
fn h3_round_trips_and_rejects_non_cells() {
    let h3 = default_h3();
    assert_eq!((h3.dims(), h3.axis_bits()), (3, 11));
    let downtown = coords(h3.encode(DOWNTOWN.0, DOWNTOWN.1));
    assert_eq!(downtown, vec![1024, 1024, 1024]);
    for p in [DOWNTOWN, AIRPORT, (30.4012, -97.7254)] {
        assert_round_trip(&h3, p);
        let c = coords(h3.encode(p.0, p.1));
        assert_eq!(h3.label(&c), h3.cell(p.0, p.1).unwrap().to_string());
    }
    // The airport's resolution 9 parent is the cell in Mastic's syn-h3.log.
    let airport = coords(h3.encode(AIRPORT.0, AIRPORT.1));
    assert_eq!(h3.cell(AIRPORT.0, AIRPORT.1).unwrap().parent(h3o::Resolution::Nine).unwrap().to_string(), "89489e30c87ffff");
    // A triple whose third coordinate is not i - j is not a cell.
    assert!(h3.decode(&[airport[0], airport[1], airport[2] + 1]).is_none());
    assert!(h3.decode(&[2048, 0, 0]).is_none());
    assert_eq!(h3.encode(FAR_AWAY.0, FAR_AWAY.1), Encoded::Dropped);
    assert_eq!(h3.encode(f64::NAN, 0.0), Encoded::Dropped);
}

/// Bounding box `((min_lat, max_lat), (min_lon, max_lon))` of polygons.
fn bbox(polygons: &[Vec<(f64, f64)>]) -> ((f64, f64), (f64, f64)) {
    let pts = polygons.iter().flatten();
    let lats = pts.clone().map(|p| p.0);
    let lons = pts.map(|p| p.1);
    (
        (lats.clone().fold(f64::MAX, f64::min), lats.fold(f64::MIN, f64::max)),
        (lons.clone().fold(f64::MAX, f64::min), lons.fold(f64::MIN, f64::max)),
    )
}

/// The region of a single cell contains its center, and the region of a box contains both its
/// corner cells.
fn assert_regions(encoder: &dyn LocationEncoder, (lat, lon): (f64, f64), cell_size: (f64, f64)) {
    let c = coords(encoder.encode(lat, lon));
    let one: Vec<(u128, u128)> = c.iter().map(|&v| (v, v)).collect();
    let ((s, n), (w, e)) = bbox(&encoder.region(&one));
    let (clat, clon) = encoder.decode(&c).unwrap();
    assert!(s < clat && clat < n && w < clon && clon < e, "center outside its region");
    assert!(((n - s) - cell_size.0).abs() < 1e-6, "cell height {}", n - s);
    assert!(((e - w) - cell_size.1).abs() < 1e-6, "cell width {}", e - w);
}

#[test]
fn rectangular_regions_are_the_cells() {
    assert_regions(&MosaicGrid, DOWNTOWN, (0.001, 0.001));
    assert_regions(&default_morton(), DOWNTOWN, (0.001, 0.001));
    assert_regions(&Geohash::new("9v6", 21), DOWNTOWN, (180.0 / (1u64 << 18) as f64, 360.0 / (1u64 << 18) as f64));
    // The whole coordinate range is clipped to the cells that exist.
    let ((s, n), _) = bbox(&MosaicGrid.region(&[(0, 2047), (0, 2047)]));
    assert!((s - 29.2665).abs() < 1e-9 && (n - 31.2665).abs() < 1e-9);
    assert!(MosaicGrid.region(&[(2000, 2047), (0, 2047)]).is_empty());
    assert!(Geohash::new("9v6", 21).region(&[(0, 2047), (1024, 2047)]).is_empty());
}

#[test]
fn h3_regions_are_hexagons_or_outlines() {
    let h3 = default_h3();
    let c = coords(h3.encode(AIRPORT.0, AIRPORT.1));
    let one: Vec<(u128, u128)> = c.iter().map(|&v| (v, v)).collect();
    let region = h3.region(&one);
    assert_eq!(region.len(), 1);
    assert_eq!(region[0].len(), 6, "a hexagon");
    let ((s, n), (w, e)) = bbox(&region);
    let (lat, lon) = h3.decode(&c).unwrap();
    assert!(s < lat && lat < n && w < lon && lon < e);
    // A box with no i - j in range has no cells.
    assert!(h3.region(&[(1024, 1024), (1024, 1024), (1030, 1035)]).is_empty());
    // A large box is one outline that contains its cells' centers.
    let big = h3.region(&[(1000, 1100), (1000, 1100), (900, 1100)]);
    assert_eq!(big.len(), 1);
    let ((s, n), (w, e)) = bbox(&big);
    let (lat, lon) = h3.decode(&[1050, 1050, 1024]).unwrap();
    assert!(s < lat && lat < n && w < lon && lon < e);
}
