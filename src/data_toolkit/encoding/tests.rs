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
