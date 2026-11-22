//! Notices boss kills as they happen, from the game's live kill flags, so the compass and the
//! list don't wait for the next sync.
//!
//! The flags in game memory belong to the world the player is in. In a friend's world they are
//! the friend's, so a flag that is merely set proves nothing there. Two things do count:
//!
//! - a flag that is set in the first world loaded after the game starts, which is always the
//!   player's own save;
//! - a flag seen unset that becomes set with no loading screen in between, which is a kill.

use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub struct Kills {
    beaten: HashSet<u32>,
    /// Flag states at the last look, in the world the player is in now.
    seen: HashMap<u32, bool>,
    /// The player has been through a loading screen since the first world.
    left_first_world: bool,
}

impl Kills {
    /// The player left the world: a loading screen, or quitting to the title.
    pub fn left_world(&mut self) {
        self.left_first_world = true;
        self.seen.clear();
    }

    /// Takes the watched flags' current states and returns the flags newly found beaten.
    pub fn observe(&mut self, flags: &[(u32, bool)]) -> Vec<u32> {
        let mut new = Vec::new();
        for &(flag, set) in flags {
            let newly_set = match self.seen.insert(flag, set) {
                Some(was_set) => !was_set,
                None => !self.left_first_world,
            };
            if set && newly_set && self.beaten.insert(flag) {
                new.push(flag);
            }
        }
        new
    }

    /// Every flag found beaten since the game started.
    pub fn beaten(&self) -> &HashSet<u32> {
        &self.beaten
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GODRICK: u32 = 10000800;
    const MARGIT: u32 = 10000850;

    #[test]
    fn a_flag_already_set_in_the_first_world_is_beaten() {
        let mut kills = Kills::default();
        assert_eq!(
            kills.observe(&[(GODRICK, true), (MARGIT, false)]),
            vec![GODRICK]
        );
    }

    #[test]
    fn a_flag_that_flips_while_in_the_world_is_beaten() {
        let mut kills = Kills::default();
        assert!(kills.observe(&[(GODRICK, false)]).is_empty());
        assert_eq!(kills.observe(&[(GODRICK, true)]), vec![GODRICK]);
    }

    #[test]
    fn a_flag_already_set_in_a_later_world_is_not_beaten() {
        // Joining a friend who has beaten Godrick: their flag, not my kill.
        let mut kills = Kills::default();
        kills.observe(&[(GODRICK, false)]);
        kills.left_world();
        assert!(kills.observe(&[(GODRICK, true)]).is_empty());
        assert!(kills.beaten().is_empty());
    }

    #[test]
    fn a_kill_in_a_later_world_is_beaten() {
        let mut kills = Kills::default();
        kills.observe(&[(GODRICK, false)]);
        kills.left_world();
        kills.observe(&[(GODRICK, false)]);
        assert_eq!(kills.observe(&[(GODRICK, true)]), vec![GODRICK]);
    }

    #[test]
    fn a_flag_unset_before_a_loading_screen_and_set_after_is_not_beaten() {
        // What was seen in one world says nothing about the next.
        let mut kills = Kills::default();
        kills.observe(&[(GODRICK, false)]);
        kills.left_world();
        kills.observe(&[(MARGIT, false)]);
        kills.left_world();
        assert!(kills.observe(&[(GODRICK, true)]).is_empty());
    }

    #[test]
    fn a_flag_first_watched_in_a_later_world_and_already_set_is_not_beaten() {
        // A boss that joins the list mid-session, after a pull.
        let mut kills = Kills::default();
        kills.observe(&[(GODRICK, false)]);
        kills.left_world();
        kills.observe(&[(GODRICK, false)]);
        assert!(kills.observe(&[(GODRICK, false), (MARGIT, true)]).is_empty());
    }

    #[test]
    fn a_kill_is_reported_once_and_remembered() {
        let mut kills = Kills::default();
        kills.observe(&[(GODRICK, false)]);
        assert_eq!(kills.observe(&[(GODRICK, true)]), vec![GODRICK]);
        assert!(kills.observe(&[(GODRICK, true)]).is_empty());
        kills.left_world();
        assert!(kills.observe(&[(GODRICK, true)]).is_empty());
        assert!(kills.beaten().contains(&GODRICK));
    }
}
