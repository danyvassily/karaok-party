use crate::sanitize_sample;

/// Telemetry stats reported by the per-microphone adaptive jitter buffer.
#[derive(Debug, Clone, Copy, Default)]
pub struct JitterStats {
    /// Current target buffer fill in milliseconds (`10.0 .. 35.0 ms`).
    pub target_delay_ms: f32,
    /// Estimated network jitter in milliseconds (RFC 3550 EWMA).
    pub estimated_jitter_ms: f32,
    /// Percentage of missing/dropped packets (`0.0 .. 100.0`).
    pub packet_loss_pct: f32,
    /// Currently buffered audio samples ready for playback.
    pub buffered_samples: usize,
}

/// Adaptive jitter buffer absorbing Wi-Fi arrival variance (10 ms .. 35 ms at 48 kHz)
/// with sequence tracking and smooth Packet Loss Concealment (PLC).
pub struct AdaptiveJitterBuffer {
    sample_rate: f32,
    ring: Vec<f32>,
    mask: usize,
    read_pos: usize,
    write_pos: usize,
    buffered: usize,
    min_target_samples: usize,
    max_target_samples: usize,
    target_samples: usize,
    primed: bool,
    last_seq: Option<u32>,
    last_arrival_us: Option<u64>,
    jitter_us: f32,
    packets_received: u64,
    packets_lost: u64,
    last_sample: f32,
}

impl AdaptiveJitterBuffer {
    const CAPACITY: usize = 8192; // ~170 ms at 48 kHz, power of 2

    pub fn new(sample_rate: f32) -> Self {
        let sr = sample_rate.max(8000.0);
        let min_target_samples = (0.010 * sr) as usize; // 10 ms = 480 samples
        let max_target_samples = (0.035 * sr) as usize; // 35 ms = 1680 samples
        let initial_target = (0.015 * sr) as usize; // 15 ms default

        Self {
            sample_rate: sr,
            ring: vec![0.0; Self::CAPACITY],
            mask: Self::CAPACITY - 1,
            read_pos: 0,
            write_pos: 0,
            buffered: 0,
            min_target_samples,
            max_target_samples,
            target_samples: initial_target,
            primed: false,
            last_seq: None,
            last_arrival_us: None,
            jitter_us: 0.0,
            packets_received: 0,
            packets_lost: 0,
            last_sample: 0.0,
        }
    }

    /// Pushes an incoming WebSocket PCM packet into the jitter buffer.
    pub fn push_packet(&mut self, seq: u32, arrival_us: u64, samples: &[f32]) {
        self.packets_received += 1;

        // Track sequence gaps for packet loss estimation and PLC fill
        if let Some(prev_seq) = self.last_seq {
            let expected = prev_seq.wrapping_add(1);
            if seq != expected {
                let gap = seq.wrapping_sub(expected);
                if gap > 0 && gap <= 8 {
                    self.packets_lost += gap as u64;
                    // Insert short smooth fade-down PLC samples for 1-2 dropped packets
                    let plc_count = (gap as usize * samples.len()).min(256);
                    for _ in 0..plc_count {
                        self.last_sample *= 0.94;
                        self.push_one_sample(self.last_sample);
                    }
                }
            }
        }
        self.last_seq = Some(seq);

        // RFC 3550-style inter-arrival jitter estimate (expected ~2667 us for 128 samples @ 48 kHz)
        let expected_interval_us = (samples.len() as f32 / self.sample_rate) * 1_000_000.0;
        if let Some(prev_us) = self.last_arrival_us {
            let delta = arrival_us.saturating_sub(prev_us) as f32;
            let deviation = (delta - expected_interval_us).abs().min(50_000.0);
            self.jitter_us += (deviation - self.jitter_us) / 16.0;

            // Adapt target buffer delay within [10ms .. 35ms]
            let desired_ms = (10.0 + (self.jitter_us / 1000.0) * 2.5).clamp(10.0, 35.0);
            self.target_samples = ((desired_ms / 1000.0) * self.sample_rate)
                .round()
                .clamp(self.min_target_samples as f32, self.max_target_samples as f32)
                as usize;
        }
        self.last_arrival_us = Some(arrival_us);

        for &s in samples {
            let clean = sanitize_sample(s);
            self.last_sample = clean;
            self.push_one_sample(clean);
        }

        // Prevent latency buildup if Wi-Fi burst overflows beyond target + 25 ms
        let max_allowed = self.target_samples + (0.025 * self.sample_rate) as usize;
        if self.buffered > max_allowed {
            let drop_n = self.buffered - self.target_samples;
            self.read_pos = (self.read_pos + drop_n) & self.mask;
            self.buffered -= drop_n;
        }

        if !self.primed && self.buffered >= self.target_samples {
            self.primed = true;
        }
    }

    #[inline]
    fn push_one_sample(&mut self, s: f32) {
        if self.buffered >= Self::CAPACITY {
            self.read_pos = (self.read_pos + 1) & self.mask;
            self.buffered -= 1;
        }
        if let Some(slot) = self.ring.get_mut(self.write_pos) {
            *slot = s;
        }
        self.write_pos = (self.write_pos + 1) & self.mask;
        self.buffered += 1;
    }

    /// Pops one sample for the downstream audio engine. Returns smooth decay on underrun.
    #[inline]
    pub fn pop_sample(&mut self) -> f32 {
        if !self.primed {
            return 0.0;
        }
        if self.buffered == 0 {
            self.primed = false;
            self.last_sample *= 0.92;
            return sanitize_sample(self.last_sample);
        }
        let s = self.ring.get(self.read_pos).copied().unwrap_or(0.0);
        self.read_pos = (self.read_pos + 1) & self.mask;
        self.buffered -= 1;
        self.last_sample = s;
        s
    }

    pub fn stats(&self) -> JitterStats {
        let total = self.packets_received + self.packets_lost;
        let loss_pct = if total > 0 {
            (self.packets_lost as f32 / total as f32) * 100.0
        } else {
            0.0
        };
        JitterStats {
            target_delay_ms: (self.target_samples as f32 / self.sample_rate) * 1000.0,
            estimated_jitter_ms: self.jitter_us / 1000.0,
            packet_loss_pct: loss_pct,
            buffered_samples: self.buffered,
        }
    }

    pub fn reset(&mut self) {
        self.read_pos = 0;
        self.write_pos = 0;
        self.buffered = 0;
        self.primed = false;
        self.last_seq = None;
        self.last_arrival_us = None;
        self.last_sample = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jitter_buffer_primes_and_adapts_within_bounds() {
        let mut jb = AdaptiveJitterBuffer::new(48_000.0);
        let frame = [0.25_f32; 128];
        let mut t_us = 0_u64;
        for seq in 0..12 {
            // Simulate 4 ms jitter bursts
            t_us += if seq % 2 == 0 { 1500 } else { 5500 };
            jb.push_packet(seq, t_us, &frame);
        }
        let stats = jb.stats();
        assert!(
            stats.target_delay_ms >= 10.0 && stats.target_delay_ms <= 35.0,
            "target_delay_ms={}",
            stats.target_delay_ms
        );
        let popped = jb.pop_sample();
        assert!((popped - 0.25).abs() < 1e-5);
    }

    #[test]
    fn jitter_buffer_tracks_dropped_packets() {
        let mut jb = AdaptiveJitterBuffer::new(48_000.0);
        let frame = [0.5_f32; 128];
        jb.push_packet(1, 0, &frame);
        jb.push_packet(3, 5334, &frame); // seq 2 lost
        let stats = jb.stats();
        assert!(stats.packet_loss_pct > 20.0);
    }
}
