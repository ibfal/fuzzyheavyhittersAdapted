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
