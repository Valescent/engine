//! Health, stamina, death, and the respawn point.
//!
//! This module is headless. Damage sources and movement live in the app and
//! player layers; the fixed tick calls [`Survival::tick`] for stamina.

use glam::Vec3;

/// Maximum health. One point is half a heart in the placeholder HUD.
pub(crate) const MAX_HEALTH: u16 = 20;
/// Maximum stamina. Stamina is an integer from 0 to 100.
pub(crate) const MAX_STAMINA: u16 = 100;
/// Sprinting drains one stamina point every 50 ms (20 per second).
const STAMINA_DRAIN_PER_SECOND: f32 = 20.0;
/// Idle regeneration restores one point every 66.7 ms (15 per second).
const STAMINA_REGEN_PER_SECOND: f32 = 15.0;

/// The player's survival state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Survival {
    health: u16,
    stamina: u16,
    dead: bool,
    spawn: Vec3,
    stamina_accum: f32,
}

impl Default for Survival {
    fn default() -> Self {
        Self {
            health: MAX_HEALTH,
            stamina: MAX_STAMINA,
            dead: false,
            spawn: Vec3::ZERO,
            stamina_accum: 0.0,
        }
    }
}

impl Survival {
    /// Full health and stamina at a respawn point.
    pub(crate) fn new(spawn: Vec3) -> Self {
        Self {
            spawn,
            ..Self::default()
        }
    }

    /// Restores a validated save state.
    pub(crate) fn from_parts(health: u16, stamina: u16, spawn: Vec3) -> Self {
        Self {
            health: health.min(MAX_HEALTH),
            stamina: stamina.min(MAX_STAMINA),
            dead: health == 0,
            spawn,
            stamina_accum: 0.0,
        }
    }

    /// Current health.
    pub(crate) fn health(&self) -> u16 {
        self.health
    }

    /// Current stamina.
    pub(crate) fn stamina(&self) -> u16 {
        self.stamina
    }

    /// Whether the player is dead and must respawn.
    #[allow(
        dead_code,
        reason = "death and respawn land in the 0.0.10.4.2 and 4.3 PRs"
    )]
    pub(crate) fn is_dead(&self) -> bool {
        self.dead
    }

    /// The respawn point.
    pub(crate) fn spawn(&self) -> Vec3 {
        self.spawn
    }

    /// Moves the respawn point.
    #[allow(
        dead_code,
        reason = "death and respawn land in the 0.0.10.4.2 and 4.3 PRs"
    )]
    pub(crate) fn set_spawn(&mut self, spawn: Vec3) {
        self.spawn = spawn;
    }

    /// Whether the player has enough stamina to start or continue sprinting.
    #[allow(
        dead_code,
        reason = "sprint wiring lands in the 0.0.10.4.2 and 4.3 PRs"
    )]
    pub(crate) fn can_sprint(&self) -> bool {
        !self.dead && self.stamina > 0
    }

    /// Applies damage. Returns `true` when this hit killed the player.
    #[allow(
        dead_code,
        reason = "damage sources land in the 0.0.10.4.2 and 4.3 PRs"
    )]
    pub(crate) fn damage(&mut self, amount: u16) -> bool {
        if self.dead || amount == 0 {
            return false;
        }
        self.health = self.health.saturating_sub(amount);
        if self.health == 0 {
            self.dead = true;
            return true;
        }
        false
    }

    /// Restores health without exceeding the maximum. Dead players stay dead.
    #[allow(
        dead_code,
        reason = "damage sources land in the 0.0.10.4.2 and 4.3 PRs"
    )]
    pub(crate) fn heal(&mut self, amount: u16) {
        if self.dead {
            return;
        }
        self.health = self.health.saturating_add(amount).min(MAX_HEALTH);
    }

    /// Restores full health and stamina after a death.
    #[allow(
        dead_code,
        reason = "death and respawn land in the 0.0.10.4.2 and 4.3 PRs"
    )]
    pub(crate) fn revive(&mut self) {
        self.health = MAX_HEALTH;
        self.stamina = MAX_STAMINA;
        self.dead = false;
        self.stamina_accum = 0.0;
    }

    /// Advances stamina on the fixed tick.
    pub(crate) fn tick(&mut self, delta: f32, sprinting: bool) {
        if delta <= 0.0 || self.dead {
            return;
        }
        let rate = if sprinting && self.stamina > 0 {
            -STAMINA_DRAIN_PER_SECOND
        } else {
            STAMINA_REGEN_PER_SECOND
        };
        self.stamina_accum += rate * delta;
        while self.stamina_accum >= 1.0 && self.stamina < MAX_STAMINA {
            self.stamina += 1;
            self.stamina_accum -= 1.0;
        }
        while self.stamina_accum <= -1.0 && self.stamina > 0 {
            self.stamina -= 1;
            self.stamina_accum += 1.0;
        }
        if self.stamina == MAX_STAMINA && self.stamina_accum > 0.0 {
            self.stamina_accum = 0.0;
        }
        if self.stamina == 0 && self.stamina_accum < 0.0 {
            self.stamina_accum = 0.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_survival_state_is_full_and_alive() {
        let state = Survival::new(Vec3::new(1.0, 2.0, 3.0));

        assert_eq!(state.health(), MAX_HEALTH);
        assert_eq!(state.stamina(), MAX_STAMINA);
        assert!(!state.is_dead());
        assert_eq!(state.spawn(), Vec3::new(1.0, 2.0, 3.0));
    }

    #[test]
    fn damage_clamps_and_kills_once() {
        let mut state = Survival::default();

        assert!(!state.damage(5));
        assert_eq!(state.health(), 15);
        assert!(state.damage(15));
        assert!(state.is_dead());
        assert_eq!(state.health(), 0);
        assert!(!state.damage(1), "a dead player ignores further damage");
    }

    #[test]
    fn healing_stops_at_the_maximum_and_does_not_revive() {
        let mut state = Survival::default();
        state.damage(10);
        state.heal(100);
        assert_eq!(state.health(), MAX_HEALTH);

        state.damage(MAX_HEALTH);
        state.heal(5);
        assert!(state.is_dead());
        assert_eq!(state.health(), 0);
    }

    #[test]
    fn revive_restores_health_and_stamina() {
        let mut state = Survival::default();
        state.damage(MAX_HEALTH);
        state.tick(1.0, true);

        state.revive();

        assert!(!state.is_dead());
        assert_eq!(state.health(), MAX_HEALTH);
        assert_eq!(state.stamina(), MAX_STAMINA);
    }

    #[test]
    fn sprinting_drains_and_idle_time_regenerates() {
        let mut state = Survival::default();

        state.tick(1.0, true);
        assert_eq!(state.stamina(), 80);
        assert!(state.can_sprint());

        state.tick(1.0, false);
        assert_eq!(state.stamina(), 95);

        state.tick(1.0, false);
        assert_eq!(state.stamina(), MAX_STAMINA);
    }

    #[test]
    fn stamina_never_drops_below_zero() {
        let mut state = Survival::default();

        state.tick(10.0, true);

        assert_eq!(state.stamina(), 0);
        assert!(!state.can_sprint());
    }

    #[test]
    fn from_parts_clamps_and_sets_the_dead_flag() {
        let state = Survival::from_parts(99, 999, Vec3::ZERO);
        assert_eq!(state.health(), MAX_HEALTH);
        assert_eq!(state.stamina(), MAX_STAMINA);

        let dead = Survival::from_parts(0, 50, Vec3::ZERO);
        assert!(dead.is_dead());
    }
}
