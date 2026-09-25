//! DropBeam patch: a deterministic Wi-Fi-like link simulator for the congestion controllers.
//!
//! Unlike the constant-link `Sim` in `mod.rs`, this one models what a home Wi-Fi LAN does to a
//! bulk QUIC transfer, and what noq's connection layer does around the controller:
//!
//! - a FIFO bottleneck with a finite drop-tail buffer, occasional medium stalls (RTT spikes),
//!   random post-bottleneck loss and A-MPDU-style delivery aggregation;
//! - a receiver that follows noq's ACK policy, including the QUIC ACK-frequency extension
//!   (`ack_eliciting_threshold`, `reordering_threshold`, `max_ack_delay`);
//! - a sender that follows noq's call order into the controller (`on_packet_sent`,
//!   `on_cwnd_limited`, per-packet `on_ack` then `on_end_acks`, RFC 9002 loss detection with
//!   packet/time thresholds and persistent congestion, `on_packet_lost` +
//!   `on_congestion_event`, spurious-loss undo, PTO probes) and paces with the controller's
//!   pacing rate like noq's `Pacer`.
use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap};

use rand::{RngExt, SeedableRng};
use rand_pcg::Pcg32;

use crate::RttEstimator;
use crate::congestion::Controller;
use crate::{Duration, Instant};

pub(super) const MSS: u16 = 1452;

#[derive(Clone, Debug)]
pub(super) struct LinkCfg {
    /// bottleneck rate, bytes/s
    pub bw: f64,
    /// base propagation RTT
    pub rtt_ms: f64,
    /// bottleneck drop-tail queue limit
    pub buf_bytes: u64,
    /// random per-packet loss after the bottleneck (air loss surviving link-layer retries)
    pub loss: f64,
    /// deliveries are released at multiples of this slot (A-MPDU aggregation)
    pub agg_ms: f64,
    /// extra random, order-preserving delay on the ACK path
    pub ack_jitter_ms: f64,
    /// expected medium stalls (the bottleneck serves nothing) per second
    pub stalls_per_s: f64,
    /// stall duration range
    pub stall_ms: (f64, f64),
    /// expected rate dips (interference, rate adaptation, a neighbour's burst) per second
    pub dips_per_s: f64,
    /// dip duration range
    pub dip_ms: (f64, f64),
    /// fraction of `bw` left during a dip, range
    pub dip_factor: (f64, f64),
    /// scripted rate dips (start s, end s, factor), in addition to the random ones
    pub forced_dips: Vec<(f64, f64, f64)>,
    /// whether the controller is told the ACK_FREQUENCY parameters (`on_ack_frequency_update`),
    /// as noq does only for the path the ACK_FREQUENCY frame was sent on
    pub tell_controller: bool,
    /// ACK_FREQUENCY ack-eliciting threshold requested of the receiver
    pub ack_thresh: u64,
    /// ACK_FREQUENCY reordering threshold
    pub reorder_thresh: u64,
    pub max_ack_delay_ms: f64,
    pub duration_s: f64,
    pub seed: u64,
}

impl LinkCfg {
    /// The measured home Wi-Fi LAN: ~40 Mbit/s, ~20 ms RTT, ~0.1% loss, RTT spikes to
    /// 100-200 ms under load, receiver asked to ACK every 11th packet.
    pub(super) fn home_wifi(seed: u64) -> Self {
        Self {
            bw: 5_000_000.0,
            rtt_ms: 20.0,
            buf_bytes: 600_000,
            loss: 0.001,
            agg_ms: 3.0,
            ack_jitter_ms: 6.0,
            stalls_per_s: 0.5,
            stall_ms: (30.0, 150.0),
            dips_per_s: 0.3,
            dip_ms: (200.0, 1500.0),
            dip_factor: (0.03, 0.3),
            forced_dips: Vec::new(),
            tell_controller: false,
            ack_thresh: 10,
            reorder_thresh: 2,
            max_ack_delay_ms: 25.0,
            duration_s: 60.0,
            seed,
        }
    }
}

#[derive(Debug, Default, Clone)]
pub(super) struct SimResult {
    pub delivered: u64,
    pub goodput_mbs: f64,
    /// fraction of 10 ms samples with cwnd at or below 4 packets
    pub pinned_frac: f64,
    /// longest continuous stretch with cwnd at or below 4 packets, seconds
    pub longest_pinned_s: f64,
    /// per-second goodput, MB/s
    pub per_sec: Vec<f64>,
    pub lost: u64,
    pub congestion_events: u64,
    pub persistent: u64,
    pub spurious: u64,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Ev {
    /// data packet reaches the receiver
    Arrive { pn: u64 },
    /// an ACK reaches the sender, covering every packet whose arrival index is below `cutoff`
    AckArrive { cutoff: u64, ack_delay_ns: u64 },
    /// receiver's delayed-ACK timer
    AckTimer { gen_: u64 },
}

struct Sent {
    time_ns: u64,
    size: u16,
}

pub(super) fn run<C: Controller + ?Sized>(
    cfg: &LinkCfg,
    ctrl: &mut C,
    mut observe: impl FnMut(&C, u64),
) -> SimResult {
    let ms = |x: f64| (x * 1e6) as u64;
    let mut rng = Pcg32::seed_from_u64(cfg.seed);
    let base = Instant::now();
    let at = |ns: u64| base + Duration::from_nanos(ns);
    let end_ns = (cfg.duration_s * 1e9) as u64;
    let one_way = ms(cfg.rtt_ms / 2.0);
    let service_ns = (MSS as f64 / cfg.bw * 1e9) as u64;
    let agg_ns = ms(cfg.agg_ms).max(1);
    let max_ack_delay_ns = ms(cfg.max_ack_delay_ms);

    // pre-draw medium stalls as (start, end), sorted
    let mut stalls = Vec::new();
    {
        let mut t = 0f64;
        while cfg.stalls_per_s > 0.0 {
            t += -(1.0 - rng.random::<f64>()).ln() / cfg.stalls_per_s;
            if t > cfg.duration_s {
                break;
            }
            let d = cfg.stall_ms.0 + rng.random::<f64>() * (cfg.stall_ms.1 - cfg.stall_ms.0);
            stalls.push(((t * 1e9) as u64, ((t + d / 1e3) * 1e9) as u64));
        }
    }
    let mut stall_idx = 0usize;
    // pre-draw rate dips as (start, end, factor), sorted and non-overlapping
    let mut dips = Vec::new();
    {
        let mut t = 0f64;
        while cfg.dips_per_s > 0.0 {
            t += -(1.0 - rng.random::<f64>()).ln() / cfg.dips_per_s;
            if t > cfg.duration_s {
                break;
            }
            let d = cfg.dip_ms.0 + rng.random::<f64>() * (cfg.dip_ms.1 - cfg.dip_ms.0);
            let f = cfg.dip_factor.0 + rng.random::<f64>() * (cfg.dip_factor.1 - cfg.dip_factor.0);
            dips.push(((t * 1e9) as u64, ((t + d / 1e3) * 1e9) as u64, f));
            t += d / 1e3;
        }
    }
    for &(a, b, f) in &cfg.forced_dips {
        dips.retain(|&(s0, e0, _)| e0 < (a * 1e9) as u64 || s0 > (b * 1e9) as u64);
        dips.push(((a * 1e9) as u64, (b * 1e9) as u64, f));
    }
    dips.sort_by_key(|d| d.0);
    let mut dip_idx = 0usize;
    if cfg.tell_controller {
        ctrl.on_ack_frequency_update(cfg.ack_thresh, Duration::from_nanos(max_ack_delay_ns));
    }

    let mut heap: BinaryHeap<Reverse<(u64, u64, Ev)>> = BinaryHeap::new();
    let mut seq = 0u64;
    let mut push = |heap: &mut BinaryHeap<Reverse<(u64, u64, Ev)>>, t: u64, ev: Ev| {
        seq += 1;
        heap.push(Reverse((t, seq, ev)));
    };

    // ---- link state
    let mut btl_free = 0u64;
    let mut last_deliver = 0u64;
    let mut last_ack_arrive = 0u64;

    // ---- receiver state (mirrors noq's PendingAcks)
    let mut arrival: Vec<u64> = Vec::new(); // pn -> arrival index (u64::MAX = not received)
    let mut arrivals = 0u64;
    let mut r_largest: Option<u64> = None;
    let mut r_largest_arrive_ns = 0u64;
    let mut r_largest_acked: Option<u64> = None; // largest pn in the last ACK we sent
    let mut r_since_ack = 0u64;
    let mut r_timer_gen = 0u64;
    let mut r_timer_armed = false;

    // ---- sender state
    let mut sent: BTreeMap<u64, Sent> = BTreeMap::new();
    let mut lost_for_spurious: BTreeMap<u64, u64> = BTreeMap::new();
    let mut in_flight = 0u64;
    let mut next_pn = 0u64;
    let mut largest_acked: Option<u64> = None;
    let mut first_pn_after_rtt_sample: Option<u64> = None;
    let mut rtt = RttEstimator::new(Duration::from_millis(333));
    let mut have_rtt = false;
    let mut loss_time: Option<u64> = None;
    let mut pto_count = 0u32;
    let mut last_eliciting_sent = 0u64;
    let mut probes_pending = 0u32;
    // pacer (noq's rate-based token bucket)
    let mut tokens = 0u64;
    let mut tokens_prev = 0u64;

    let mut now = 0u64;
    let mut res = SimResult::default();
    let mut delivered = 0u64;
    let mut sec_mark = 0u64;
    let mut sec_bytes = 0u64;
    let mut next_sample = 0u64;
    let (mut pinned, mut samples, mut run_start): (u64, u64, Option<u64>) = (0, 0, None);
    let mut longest = 0u64;
    let pin_level = 4 * MSS as u64;

    let pto_base = |rtt: &RttEstimator| rtt.pto_base().as_nanos() as u64 + max_ack_delay_ns;

    while now < end_ns {
        // ---------------- sender: send whatever cwnd + pacer allow at `now`
        loop {
            let probe = probes_pending > 0;
            if !probe && in_flight + MSS as u64 > ctrl.window() {
                ctrl.on_cwnd_limited();
                break;
            }
            if !probe {
                // noq Pacer::delay_at_rate / window fallback
                let m = ctrl.metrics();
                let rate = m.pacing_rate.unwrap_or_else(|| {
                    (ctrl.window() as f64 * 1.25 / rtt.get().as_secs_f64()) as u64
                });
                let rate = rate.max(1);
                let bytes_in = |ns: u64| (rate as u128 * ns as u128 / 1_000_000_000) as u64;
                let cap = Ord::min(
                    Ord::max(bytes_in(ms(10.0)), MSS as u64),
                    bytes_in(ms(2.0)).clamp(10 * MSS as u64, 256 * MSS as u64),
                );
                let nt = bytes_in(now - tokens_prev);
                if nt > 0 {
                    tokens = (tokens + nt).min(cap);
                    tokens_prev = now;
                }
                tokens = tokens.min(cap);
                if tokens < MSS as u64 {
                    break;
                }
                tokens -= MSS as u64;
            } else {
                probes_pending -= 1;
            }
            // transmit
            let pn = next_pn;
            next_pn += 1;
            ctrl.on_packet_sent(at(now), MSS, pn);
            ctrl.on_sent(at(now), MSS as u64, pn);
            in_flight += MSS as u64;
            sent.insert(pn, Sent { time_ns: now, size: MSS });
            last_eliciting_sent = now;
            arrival.push(u64::MAX);
            // bottleneck
            let arr = now + ms(0.5);
            let queued = btl_free.saturating_sub(arr) as f64 * cfg.bw / 1e9;
            if queued as u64 + MSS as u64 > cfg.buf_bytes {
                continue; // tail drop
            }
            let mut start = arr.max(btl_free);
            while stall_idx < stalls.len() && stalls[stall_idx].1 <= start {
                stall_idx += 1;
            }
            if stall_idx < stalls.len() && stalls[stall_idx].0 <= start {
                start = stalls[stall_idx].1;
            }
            while dip_idx < dips.len() && dips[dip_idx].1 <= start {
                dip_idx += 1;
            }
            let service = match dips.get(dip_idx) {
                Some(&(s0, _, f)) if s0 <= start => (service_ns as f64 / f) as u64,
                _ => service_ns,
            };
            let finish = start + service;
            btl_free = finish;
            if rng.random_bool(cfg.loss) {
                continue;
            }
            let t = (finish + one_way).div_ceil(agg_ns) * agg_ns;
            let t = t.max(last_deliver);
            last_deliver = t;
            push(&mut heap, t, Ev::Arrive { pn });
        }

        // ---------------- next event
        let t_heap = heap.peek().map(|Reverse((t, _, _))| *t).unwrap_or(u64::MAX);
        let t_pace = if in_flight + MSS as u64 <= ctrl.window() {
            let m = ctrl.metrics();
            let rate = m.pacing_rate.unwrap_or_else(|| {
                (ctrl.window() as f64 * 1.25 / rtt.get().as_secs_f64()) as u64
            });
            let deficit = (MSS as u64).saturating_sub(tokens);
            now + (deficit as f64 / rate.max(1) as f64 * 1e9) as u64 + 1
        } else {
            u64::MAX
        };
        let t_loss = loss_time.unwrap_or(u64::MAX);
        let t_pto = if loss_time.is_none() && in_flight > 0 {
            last_eliciting_sent + (pto_base(&rtt) << pto_count.min(10))
        } else {
            u64::MAX
        };
        let t_next = t_heap.min(t_pace).min(t_loss).min(t_pto).min(next_sample);
        if t_next == u64::MAX {
            break;
        }
        now = now.max(t_next);

        if now >= next_sample {
            samples += 1;
            let w = ctrl.window();
            if w <= pin_level {
                pinned += 1;
                run_start.get_or_insert(now);
            } else if let Some(s) = run_start.take() {
                longest = longest.max(now - s);
            }
            observe(ctrl, now);
            next_sample += ms(10.0);
            if now - sec_mark >= 1_000_000_000 {
                res.per_sec.push(sec_bytes as f64 / 1e6);
                sec_bytes = 0;
                sec_mark = now;
            }
        }

        // loss timer
        let mut run_detect = false;
        let mut due_to_ack = false;
        if loss_time.is_some_and(|t| t <= now) {
            loss_time = None;
            run_detect = true;
        } else if t_pto <= now && t_pto != u64::MAX {
            pto_count += 1;
            probes_pending = 2;
        }

        // heap events
        while let Some(Reverse((t, _, _))) = heap.peek() {
            if *t > now {
                break;
            }
            let Reverse((_, _, ev)) = heap.pop().unwrap();
            match ev {
                Ev::Arrive { pn } => {
                    if arrival[pn as usize] != u64::MAX {
                        continue;
                    }
                    arrival[pn as usize] = arrivals;
                    arrivals += 1;
                    let prev_largest = r_largest.unwrap_or(0);
                    if r_largest.is_none_or(|l| pn > l) {
                        r_largest = Some(pn);
                        r_largest_arrive_ns = now;
                    }
                    let _ = prev_largest;
                    r_since_ack += 1;
                    let mut immediate = r_since_ack > cfg.ack_thresh;
                    // noq is_out_of_order, reordering_threshold >= 2
                    if let (Some(la), Some(lu)) = (r_largest_acked, r_largest)
                        && cfg.reorder_thresh <= la
                    {
                        let from = la - cfg.reorder_thresh + 1;
                        if let Some(missing) =
                            (from..=lu).find(|p| arrival[*p as usize] == u64::MAX)
                        {
                            immediate |= lu - missing >= cfg.reorder_thresh;
                        }
                    }
                    if immediate {
                        let ad = now - r_largest_arrive_ns;
                        let t = (now + one_way + (rng.random::<f64>() * ms(cfg.ack_jitter_ms) as f64) as u64)
                            .max(last_ack_arrive);
                        last_ack_arrive = t;
                        push(&mut heap, t, Ev::AckArrive { cutoff: arrivals, ack_delay_ns: ad });
                        r_largest_acked = r_largest;
                        r_since_ack = 0;
                        r_timer_armed = false;
                        r_timer_gen += 1;
                    } else if !r_timer_armed {
                        r_timer_armed = true;
                        r_timer_gen += 1;
                        push(&mut heap, now + max_ack_delay_ns, Ev::AckTimer { gen_: r_timer_gen });
                    }
                }
                Ev::AckTimer { gen_ } => {
                    if gen_ != r_timer_gen || !r_timer_armed || r_since_ack == 0 {
                        continue;
                    }
                    let ad = now - r_largest_arrive_ns;
                    let t = (now + one_way + (rng.random::<f64>() * ms(cfg.ack_jitter_ms) as f64) as u64)
                        .max(last_ack_arrive);
                    last_ack_arrive = t;
                    push(&mut heap, t, Ev::AckArrive { cutoff: arrivals, ack_delay_ns: ad });
                    r_largest_acked = r_largest;
                    r_since_ack = 0;
                    r_timer_armed = false;
                    r_timer_gen += 1;
                }
                Ev::AckArrive { cutoff, ack_delay_ns } => {
                    let is_acked = |pn: u64| arrival[pn as usize] < cutoff;
                    // largest acknowledged in this frame
                    let frame_largest = (0..next_pn).rev().find(|p| is_acked(*p));
                    let Some(frame_largest) = frame_largest else { continue };
                    let new_largest = largest_acked.is_none_or(|l| frame_largest > l);
                    if new_largest {
                        largest_acked = Some(frame_largest);
                    }
                    // spurious loss detection (noq detect_spurious_loss)
                    if !lost_for_spurious.is_empty() {
                        lost_for_spurious.retain(|pn, _| !is_acked(*pn));
                        if lost_for_spurious.is_empty() {
                            res.spurious += 1;
                            ctrl.on_spurious_congestion_event();
                        }
                    }
                    let newly: Vec<u64> = sent.keys().copied().filter(|p| is_acked(*p)).collect();
                    if newly.is_empty() {
                        continue;
                    }
                    let largest_sent_time = sent.get(&frame_largest).map(|s| s.time_ns);
                    for pn in &newly {
                        let s = sent.remove(pn).unwrap();
                        in_flight -= s.size as u64;
                        delivered += s.size as u64;
                        sec_bytes += s.size as u64;
                        ctrl.on_ack(at(now), at(s.time_ns), s.size as u64, *pn, false, &rtt);
                    }
                    ctrl.on_end_acks(at(now), in_flight, false, largest_acked);
                    if new_largest && let Some(st) = largest_sent_time {
                        let ad = Duration::from_nanos(ack_delay_ns.min(max_ack_delay_ns));
                        rtt.update(ad, Duration::from_nanos(now - st));
                        have_rtt = true;
                        if first_pn_after_rtt_sample.is_none() {
                            first_pn_after_rtt_sample = Some(next_pn);
                        }
                    }
                    run_detect = true;
                    due_to_ack = true;
                    pto_count = 0;
                }
            }
        }

        if run_detect && let Some(la) = largest_acked {
            // noq detect_lost_packets
            let loss_delay = (rtt.conservative().as_nanos() as f64 * 9.0 / 8.0).max(1e6) as u64;
            let congestion_period = pto_base(&rtt) * 3;
            let mut lost = Vec::new();
            let mut pc_start: Option<u64> = None;
            let mut persistent = false;
            let mut prev: Option<u64> = None;
            loss_time = None;
            for (&pn, s) in sent.range(..la) {
                if prev != Some(pn.wrapping_sub(1)) {
                    pc_start = None;
                }
                if now.saturating_sub(s.time_ns) >= loss_delay || la >= pn + 3 {
                    lost.push(pn);
                    if due_to_ack && have_rtt {
                        match pc_start {
                            Some(st) if s.time_ns - st > congestion_period => persistent = true,
                            None if first_pn_after_rtt_sample.is_some_and(|x| x < pn) => {
                                pc_start = Some(s.time_ns)
                            }
                            _ => {}
                        }
                    }
                } else {
                    if loss_time.is_none() {
                        loss_time = Some(s.time_ns + loss_delay);
                    }
                    pc_start = None;
                }
                prev = Some(pn);
            }
            // drain_lost_packets
            let two_pto = 2 * rtt.pto_base().as_nanos() as u64;
            lost_for_spurious.retain(|_, t| now.saturating_sub(*t) <= two_pto);
            if let Some(&largest_lost) = lost.last() {
                let largest_lost_sent = sent[&largest_lost].time_ns;
                let mut bytes = 0;
                for pn in &lost {
                    let s = sent.remove(pn).unwrap();
                    in_flight -= s.size as u64;
                    bytes += s.size as u64;
                    ctrl.on_packet_lost(s.size, *pn, at(now));
                    lost_for_spurious.insert(*pn, s.time_ns);
                }
                res.lost += lost.len() as u64;
                res.congestion_events += 1;
                if persistent {
                    res.persistent += 1;
                }
                ctrl.on_congestion_event(
                    at(now),
                    at(largest_lost_sent),
                    persistent,
                    false,
                    bytes,
                    largest_lost,
                );
            }
        }
    }
    if let Some(s) = run_start {
        longest = longest.max(now - s);
    }
    res.delivered = delivered;
    res.goodput_mbs = delivered as f64 / 1e6 / cfg.duration_s;
    res.pinned_frac = pinned as f64 / samples.max(1) as f64;
    res.longest_pinned_s = longest as f64 / 1e9;
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::congestion::bbr3::{Bbr3, Bbr3Config};
    use crate::congestion::{ControllerFactory, CubicConfig, NewRenoConfig};
    use std::sync::Arc;

    fn bbr(seed: u64, fix: bool) -> Bbr3 {
        let mut c = Bbr3Config::default();
        c.initial_window(128 * 1024);
        c.dropbeam_fixes(fix);
        c.probe_rng_seed = Some([seed as u8; 16]);
        Bbr3::new(Arc::new(c), MSS)
    }

    /// Seconds (whole, from `from_s` on) whose goodput was below 1 MB/s, a fifth of the link.
    fn slow_secs(r: &SimResult, from_s: usize) -> usize {
        r.per_sec.iter().skip(from_s).filter(|x| **x < 1.0).count()
    }

    fn fmt(r: &SimResult, from_s: usize) -> String {
        format!(
            "goodput {:4.2} MB/s  slow(<1MB/s) {:2}s  cwnd<=4pkt {:4.1}% (longest {:4.1}s)  lost {}",
            r.goodput_mbs,
            slow_secs(r, from_s),
            r.pinned_frac * 100.0,
            r.longest_pinned_s,
            r.lost,
        )
    }

    /// Clean 40 Mbit/s, 5 ms LAN link at 0.1% loss, receiver asked to ACK every 11th packet,
    /// with one scripted 3 s deep fade (3% of the rate) at t = 5..8 s.
    fn fade_cfg(seed: u64, tell: bool) -> LinkCfg {
        let mut cfg = LinkCfg::home_wifi(seed);
        cfg.rtt_ms = 5.0;
        cfg.stalls_per_s = 0.0;
        cfg.dips_per_s = 0.0;
        cfg.forced_dips = vec![(5.0, 8.0, 0.03)];
        cfg.tell_controller = tell;
        cfg.duration_s = 40.0;
        cfg
    }

    /// Stock noq BBRv3 stays in the delayed-ACK trap long after a fade: goodput sits at
    /// ~0.1-0.2 MB/s (the field symptom) for many seconds, and can fall back into it with no
    /// fade at all. With the DropBeam fixes it is back at link speed within ~1 s.
    #[test]
    fn wifi_fade_recovers_with_fixes() {
        let (mut stock_slow, mut fixed_slow) = (0, 0);
        for seed in 1..=4u64 {
            for tell in [false, true] {
                let cfg = fade_cfg(seed, tell);
                let stock = run(&cfg, &mut bbr(seed, false), |_, _| {});
                let fixed = run(&cfg, &mut bbr(seed, true), |_, _| {});
                // whole seconds 9.. are after the fade (t = 8 s) plus one second of recovery
                stock_slow += slow_secs(&stock, 9);
                fixed_slow += slow_secs(&fixed, 9);
                println!("seed {seed} tell={tell} stock {}", fmt(&stock, 9));
                println!("seed {seed} tell={tell} fixed {}", fmt(&fixed, 9));
                if tell {
                    // informed of the ACK-frequency threshold: never trapped
                    assert!(slow_secs(&fixed, 9) <= 1, "seed {seed}: {}", fmt(&fixed, 9));
                    assert!(fixed.goodput_mbs > 4.0, "seed {seed}: {}", fmt(&fixed, 9));
                }
            }
        }
        println!("slow seconds after the fade, all runs: stock {stock_slow}, fixed {fixed_slow}");
        assert!(stock_slow >= 20, "the stock trap no longer reproduces ({stock_slow})");
        assert!(fixed_slow * 4 <= stock_slow, "stock {stock_slow} fixed {fixed_slow}");
    }

    /// Randomized home Wi-Fi (stalls, rate dips, 0.1% loss, ACK every 11th packet), all
    /// controllers. Informational: `cargo test wifi_report -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn wifi_report() {
        for rtt in [5.0, 20.0] {
            let mut tot = [0f64; 5];
            for seed in 1..=6u64 {
                let mut cfg = LinkCfg::home_wifi(seed);
                cfg.rtt_ms = rtt;
                let mut line = |name: &str, i: usize, r: SimResult| {
                    tot[i] += r.goodput_mbs / 6.0;
                    println!("rtt {rtt:>2} seed {seed} {name:<16} {}", fmt(&r, 0));
                };
                let mut informed = cfg.clone();
                informed.tell_controller = true;
                line("bbr stock", 0, run(&cfg, &mut bbr(seed, false), |_, _| {}));
                line("bbr fixed", 1, run(&cfg, &mut bbr(seed, true), |_, _| {}));
                line("bbr fixed+told", 2, run(&informed, &mut bbr(seed, true), |_, _| {}));
                let mut c = CubicConfig::default();
                c.initial_window(128 * 1024);
                let mut c = Arc::new(c).build(Instant::now(), MSS);
                line("cubic", 3, run(&cfg, &mut *c, |_, _| {}));
                let mut c = NewRenoConfig::default();
                c.initial_window(128 * 1024);
                let mut c = Arc::new(c).build(Instant::now(), MSS);
                line("newreno", 4, run(&cfg, &mut *c, |_, _| {}));
            }
            println!(
                "rtt {rtt} MEAN goodput MB/s: bbr stock {:.2} | bbr fixed {:.2} | bbr fixed+told {:.2} | cubic {:.2} | newreno {:.2}",
                tot[0], tot[1], tot[2], tot[3], tot[4]
            );
        }
    }
}
