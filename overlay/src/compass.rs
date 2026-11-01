//! Where bosses and the player are on the overworld, and which way to point.
//!
//! The overworld (map area 60) is a grid of 256 m tiles named m60_XX_ZZ_00, each with its local
//! origin at its center; X grows east and Z grows north. Everything here works in "global"
//! overworld coordinates: tile × 256 + position within the tile.
//!
//! Boss kill flags encode the map the boss is in. Overworld bosses (flag 10XXZZnnnn) give their
//! tile, so their position is known to within about 128 m. Dungeon bosses (flag AABBnnnn) give the
//! dungeon map mAA_BB; the game's WorldMapLegacyConvParam says where each dungeon sits on the
//! overworld, which is how the in-game map places them.

/// A map ID like m60_42_36_00 or m10_00_00_00.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MapId {
    pub area: u8,
    pub x: u8,
    pub z: u8,
    /// Overworld tile size: 0 is 256 m, 1 is 512 m, 2 is 1024 m.
    pub index: u8,
}

impl std::fmt::Display for MapId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "m{:02}_{:02}_{:02}_{:02}",
            self.area, self.x, self.z, self.index
        )
    }
}

/// The map a boss is in, from its kill flag.
pub fn boss_map(flag: u32) -> Option<MapId> {
    let digits = flag.to_string();
    let part = |range: std::ops::Range<usize>| digits.get(range)?.parse::<u8>().ok();
    match digits.len() {
        // 10XXZZnnnn: overworld tile m60_XX_ZZ_00.
        10 if digits.starts_with("10") => Some(MapId {
            area: 60,
            x: part(2..4)?,
            z: part(4..6)?,
            index: 0,
        }),
        // AABBnnnn: dungeon mAA_BB_00_00.
        8 => Some(MapId {
            area: part(0..2)?,
            x: part(2..4)?,
            z: 0,
            index: 0,
        }),
        _ => None,
    }
}

/// One row of WorldMapLegacyConvParam: a point in a dungeon map and the same point on the
/// overworld.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ConvRow {
    pub src: MapId,
    pub src_pos: [f32; 3],
    pub dst: MapId,
    pub dst_pos: [f32; 3],
    pub base_point: bool,
}

/// Global (x, z) of a position inside an overworld tile.
fn tile_global(map: MapId, local_x: f32, local_z: f32) -> (f32, f32) {
    let scale = 256.0 * (1u32 << map.index.min(2)) as f32;
    let center = |n: u8| n as f32 * scale + (scale - 256.0) / 2.0;
    (center(map.x) + local_x, center(map.z) + local_z)
}

/// The conversion row for a dungeon map, preferring the one marked as its base point.
fn conv_for(map: MapId, conv: &[ConvRow]) -> Option<&ConvRow> {
    let matching = || {
        conv.iter().filter(move |r| {
            r.dst.area == 60 && (r.src.area, r.src.x, r.src.z) == (map.area, map.x, map.z)
        })
    };
    matching()
        .find(|r| r.base_point)
        .or_else(|| matching().next())
}

/// Global (x, z) of a position in any map, if that map can be placed on the overworld.
pub fn to_global(map: MapId, local: [f32; 3], conv: &[ConvRow]) -> Option<(f32, f32)> {
    if map.area == 60 {
        return Some(tile_global(map, local[0], local[2]));
    }
    let row = conv_for(map, conv)?;
    Some(tile_global(
        row.dst,
        row.dst_pos[0] + local[0] - row.src_pos[0],
        row.dst_pos[2] + local[2] - row.src_pos[2],
    ))
}

/// Global (x, z) of a boss: its tile's center, or where its dungeon sits on the overworld.
pub fn boss_global(flag: u32, conv: &[ConvRow]) -> Option<(f32, f32)> {
    let map = boss_map(flag)?;
    if map.area == 60 {
        return Some(tile_global(map, 0.0, 0.0));
    }
    let row = conv_for(map, conv)?;
    Some(tile_global(row.dst, row.dst_pos[0], row.dst_pos[2]))
}

/// Direction and distance from the player to a target. The angle is in radians relative to where
/// the camera faces: 0 is straight ahead, positive is to the right.
pub fn bearing(player: (f32, f32), target: (f32, f32), forward: (f32, f32)) -> (f32, f32) {
    let (dx, dz) = (target.0 - player.0, target.1 - player.1);
    let distance = (dx * dx + dz * dz).sqrt();
    let (fx, fz) = forward;
    // X east, Z north, Y up: "right" of a forward vector (fx, fz) is (fz, -fx).
    let angle = (dx * fz - dz * fx).atan2(dx * fx + dz * fz);
    (angle, distance)
}

/// Boss positions are only known to the tile (about 128 m), so closer than this the arrow
/// isn't meaningful.
pub const NEARBY: f32 = 150.0;

/// "~620 m", "~1.4 km", or "nearby".
pub fn distance_text(meters: f32) -> String {
    if meters < NEARBY {
        "nearby".into()
    } else if meters < 1000.0 {
        format!("~{} m", (meters / 10.0).round() as u32 * 10)
    } else {
        format!("~{:.1} km", meters / 1000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::FRAC_PI_2;

    fn map(area: u8, x: u8, z: u8) -> MapId {
        MapId {
            area,
            x,
            z,
            index: 0,
        }
    }

    #[test]
    fn maps_from_flags() {
        // Tree Sentinel, by the Church of Elleh.
        assert_eq!(boss_map(1042360800), Some(map(60, 42, 36)));
        // Margit, in Stormveil Castle.
        assert_eq!(boss_map(10000850), Some(map(10, 0, 0)));
        // Groveside Cave.
        assert_eq!(boss_map(31030800), Some(map(31, 3, 0)));
        // Night's Cavalry Duo: an encoding we don't know.
        assert_eq!(boss_map(1248550800), None);
        assert_eq!(boss_map(71190), None);
        assert_eq!(map(60, 42, 36).to_string(), "m60_42_36_00");
    }

    #[test]
    fn overworld_positions() {
        assert_eq!(
            boss_global(1042360800, &[]),
            Some((42.0 * 256.0, 36.0 * 256.0))
        );
        assert_eq!(
            to_global(map(60, 42, 36), [10.0, 99.0, -20.0], &[]),
            Some((10762.0, 9196.0))
        );
        // A medium tile covers four small ones; its center is between them.
        let medium = MapId {
            area: 60,
            x: 21,
            z: 18,
            index: 1,
        };
        assert_eq!(
            to_global(medium, [0.0; 3], &[]),
            Some((42.5 * 256.0, 36.5 * 256.0))
        );
    }

    #[test]
    fn dungeons_go_through_the_conversion_table() {
        let conv = [
            ConvRow {
                src: map(10, 0, 0),
                src_pos: [100.0, 0.0, 50.0],
                dst: map(60, 43, 37),
                dst_pos: [-20.0, 0.0, 30.0],
                base_point: false,
            },
            ConvRow {
                src: map(10, 0, 0),
                src_pos: [0.0, 0.0, 0.0],
                dst: map(60, 43, 38),
                dst_pos: [5.0, 0.0, 5.0],
                base_point: true,
            },
            ConvRow {
                src: map(31, 3, 0),
                src_pos: [0.0; 3],
                dst: map(61, 1, 1),
                dst_pos: [0.0; 3],
                base_point: true,
            },
        ];
        // Margit: Stormveil's base point.
        assert_eq!(
            boss_global(10000850, &conv),
            Some((43.0 * 256.0 + 5.0, 38.0 * 256.0 + 5.0))
        );
        // The player 10 m east of that point inside Stormveil.
        assert_eq!(
            to_global(map(10, 0, 0), [10.0, 0.0, 0.0], &conv),
            Some((43.0 * 256.0 + 15.0, 38.0 * 256.0 + 5.0))
        );
        // Only rows that land on the base-game overworld count.
        assert_eq!(boss_global(31030800, &conv), None);
        // Underground maps aren't in the table.
        assert_eq!(to_global(map(12, 2, 0), [0.0; 3], &conv), None);
    }

    #[test]
    fn bearings() {
        let north = (0.0, 1.0);
        let close = |a: f32, b: f32| (a - b).abs() < 1e-4;
        let (angle, distance) = bearing((0.0, 0.0), (0.0, 500.0), north);
        assert!(close(angle, 0.0) && close(distance, 500.0));
        assert!(close(bearing((0.0, 0.0), (300.0, 0.0), north).0, FRAC_PI_2));
        assert!(close(
            bearing((0.0, 0.0), (-300.0, 0.0), north).0,
            -FRAC_PI_2
        ));
        // Facing east, a target to the north is on the left.
        assert!(close(
            bearing((0.0, 0.0), (0.0, 300.0), (1.0, 0.0)).0,
            -FRAC_PI_2
        ));
    }

    #[test]
    fn distances() {
        assert_eq!(distance_text(90.0), "nearby");
        assert_eq!(distance_text(617.0), "~620 m");
        assert_eq!(distance_text(1449.0), "~1.4 km");
    }
}
