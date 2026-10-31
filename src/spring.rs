//! Damped spring used for the island morph. Underdamped (ratio ~0.68) so it overshoots slightly.

const STIFFNESS: f32 = 420.0;
const DAMPING: f32 = 28.0;
const SUBSTEP: f32 = 0.004;
const MAX_DT: f32 = 0.05;

#[derive(Clone, Copy, Default)]
pub struct Spring {
    pub pos: f32,
    pub vel: f32,
}

impl Spring {
    pub fn step(&mut self, target: f32, dt: f32) {
        let dt = dt.clamp(0.0, MAX_DT);
        let n = ((dt / SUBSTEP).ceil() as u32).max(1);
        let h = dt / n as f32;
        for _ in 0..n {
            let accel = -STIFFNESS * (self.pos - target) - DAMPING * self.vel;
            self.vel += accel * h;
            self.pos += self.vel * h;
        }
    }

    pub fn settled(&self, target: f32) -> bool {
        (self.pos - target).abs() < 0.001 && self.vel.abs() < 0.01
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converges_with_slight_overshoot() {
        let mut s = Spring::default();
        let mut peak = 0.0f32;
        for _ in 0..125 {
            s.step(1.0, 0.016);
            peak = peak.max(s.pos);
        }
        assert!(s.settled(1.0), "pos={} vel={}", s.pos, s.vel);
        assert!(peak > 1.0 && peak < 1.1, "peak={peak}");
    }
}
