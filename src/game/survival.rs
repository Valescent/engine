//! Health, stamina, death, and the respawn point.
//!
//! This module is headless. Damage sources and movement live in the app and
//! player layers; the fixed tick calls [`Survival::tick`] for stamina.

use glam::Vec3;

/// Maximum health. One point is half a heart in the placeholder HUD.
pub(crate) const MAX_HEALTH: u16 = 20;
/// Maximum stamina. Stamina is an integer from 0 to 100.
pub(crate) const MAX_STAMINA: u16 = 100;
/// Seconds of air before drowning damage starts.
const MAX_AIR: f32 = 15.0;
/// Damage per second after the air runs out.
const DROWN_DAMAGE: u16 = 1;
/// Lava damage interval in seconds.
const LAVA_DAMAGE_INTERVAL: f32 = 0.5;
/// Damage per lava tick.
const LAVA_DAMAGE: u16 = 4;
/// Fire damage interval in seconds.
const FIRE_DAMAGE_INTERVAL: f32 = 1.0;
/// Damage per fire tick.
const FIRE_DAMAGE: u16 = 1;
/// Fall distance that causes no damage.
const SAFE_FALL_DISTANCE: f32 = 3.0;
/// Stamina spent by a jump.
const JUMP_STAMINA_COST: u16 = 5;
/// Sprinting drains one stamina point every 50 ms (20 per second).
const STAMINA_DRAIN_PER_SECOND: f32 = 20.0;
/// Idle regeneration restores one point every 66.7 ms (15 per second).
const STAMINA_REGEN_PER_SECOND: f32 = 15.0;

/// Fall damage for a landing distance in blocks.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub(crate) fn fall_damage(distance: f32) -> u16 {
    if distance <= SAFE_FALL_DISTANCE {
        return 0;
    }
    ((distance - SAFE_FALL_DISTANCE).floor() as u16).min(MAX_HEALTH)
}

/// The player's survival state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Survival {
    health: u16,
    stamina: u16,
    dead: bool,
    spawn: Vec3,
    stamina_accum: f32,
    air: f32,
    drown_accum: f32,
    lava_accum: f32,
    fire_accum: f32,
}

impl Default for Survival {
    fn default() -> Self {
        Self {
            health: MAX_HEALTH,
            stamina: MAX_STAMINA,
            dead: false,
            spawn: Vec3::ZERO,
            stamina_accum: 0.0,
            air: MAX_AIR,
            drown_accum: 0.0,
            lava_accum: 0.0,
            fire_accum: 0.0,
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
            air: MAX_AIR,
            drown_accum: 0.0,
            lava_accum: 0.0,
            fire_accum: 0.0,
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

    /// Remaining air in seconds.
    #[allow(dead_code, reason = "the survival HUD reads air in 0.0.10.4.3")]
    pub(crate) fn air(&self) -> f32 {
        self.air
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
        reason = "respawn wiring lands in the 0.0.10.4.2 and 4.3 PRs"
    )]
    pub(crate) fn revive(&mut self) {
        self.health = MAX_HEALTH;
        self.stamina = MAX_STAMINA;
        self.dead = false;
        self.stamina_accum = 0.0;
        self.air = MAX_AIR;
        self.drown_accum = 0.0;
        self.lava_accum = 0.0;
        self.fire_accum = 0.0;
    }

    /// Spends the jump stamina cost. Returns `false` when the jump is refused.
    pub(crate) fn spend_jump(&mut self) -> bool {
        if self.dead || self.stamina < JUMP_STAMINA_COST {
            return false;
        }
        self.stamina -= JUMP_STAMINA_COST;
        true
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

    /// Advances air, drowning, lava, and fire timers. Returns damage to apply.
    pub(crate) fn tick_environment(
        &mut self,
        delta: f32,
        head_in_water: bool,
        in_lava: bool,
        on_fire: bool,
    ) -> u16 {
        if delta <= 0.0 || self.dead {
            return 0;
        }
        let mut damage = 0_u16;

        if head_in_water {
            self.air = (self.air - delta).max(0.0);
            if self.air == 0.0 {
                self.drown_accum += delta;
                while self.drown_accum >= 1.0 {
                    damage = damage.saturating_add(DROWN_DAMAGE);
                    self.drown_accum -= 1.0;
                }
            }
        } else {
            self.air = (self.air + delta * 4.0).min(MAX_AIR);
            self.drown_accum = 0.0;
        }

        if in_lava {
            self.lava_accum += delta;
            while self.lava_accum >= LAVA_DAMAGE_INTERVAL {
                damage = damage.saturating_add(LAVA_DAMAGE);
                self.lava_accum -= LAVA_DAMAGE_INTERVAL;
            }
        } else {
            self.lava_accum = 0.0;
        }

        if on_fire {
            self.fire_accum += delta;
            while self.fire_accum >= FIRE_DAMAGE_INTERVAL {
                damage = damage.saturating_add(FIRE_DAMAGE);
                self.fire_accum -= FIRE_DAMAGE_INTERVAL;
            }
        } else {
            self.fire_accum = 0.0;
        }

        damage
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

    #[test]
    fn fall_damage_ignores_the_first_three_blocks() {
        assert_eq!(fall_damage(0.0), 0);
        assert_eq!(fall_damage(3.0), 0);
        assert_eq!(fall_damage(3.9), 0);
        assert_eq!(fall_damage(4.0), 1);
        assert_eq!(fall_damage(10.5), 7);
        assert_eq!(fall_damage(100.0), MAX_HEALTH);
    }

    #[test]
    fn jumping_costs_stamina_and_is_refused_when_exhausted() {
        let mut state = Survival::default();
        assert!(state.spend_jump());
        assert_eq!(state.stamina(), MAX_STAMINA - 5);

        state.tick(10.0, true);
        assert!(!state.spend_jump());
    }

    #[test]
    fn drowning_starts_after_fifteen_seconds_and_damages_each_second() {
        let mut state = Survival::default();
        assert_eq!(state.tick_environment(14.0, true, false, false), 0);
        assert!(state.air() > 0.0);

        assert_eq!(state.tick_environment(1.0, true, false, false), 1);
        assert_eq!(state.air(), 0.0);
        assert_eq!(state.tick_environment(1.0, true, false, false), 1);
    }

    #[test]
    fn leaving_water_refills_air() {
        let mut state = Survival::default();
        state.tick_environment(15.0, true, false, false);
        assert_eq!(state.air(), 0.0);

        state.tick_environment(1.0, false, false, false);
        assert!(state.air() >= 4.0);
    }

    #[test]
    fn lava_and_fire_use_their_own_intervals() {
        let mut state = Survival::default();

        assert_eq!(state.tick_environment(0.5, false, true, false), 4);
        assert_eq!(state.tick_environment(0.25, false, true, false), 0);
        assert_eq!(state.tick_environment(0.25, false, true, false), 4);

        assert_eq!(state.tick_environment(1.0, false, false, true), 1);
        assert_eq!(state.tick_environment(0.5, false, false, false), 0);
    }
}
