//! How many connections the server holds at once (R3): per peer address and
//! in all. Every accepted connection costs a task and buffers before it has
//! proved anything, so without a limit one host could open them until the
//! server ran out of memory or file handles.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};

/// At most this many connections from one address at a time.
pub const PER_ADDRESS: usize = 8;
/// At most this many connections in all.
pub const TOTAL: usize = 128;

#[derive(Debug, Default)]
struct Counts {
    total: usize,
    by_ip: HashMap<IpAddr, usize>,
}

/// A connection budget, shared by the accept loops.
#[derive(Debug, Clone)]
pub struct ConnBudget {
    per_address: usize,
    total: usize,
    counts: Arc<Mutex<Counts>>,
}

/// A connection's place in the budget, given back when it is dropped.
#[derive(Debug)]
pub struct Slot {
    ip: IpAddr,
    counts: Arc<Mutex<Counts>>,
}

impl ConnBudget {
    pub fn new(per_address: usize, total: usize) -> Self {
        Self {
            per_address,
            total,
            counts: Arc::default(),
        }
    }

    /// A slot for a connection from `ip`, or `None` when that address or
    /// the server is at its limit.
    pub fn admit(&self, ip: IpAddr) -> Option<Slot> {
        let mut counts = self.counts.lock().unwrap_or_else(|e| e.into_inner());
        let from_ip = counts.by_ip.get(&ip).copied().unwrap_or(0);
        if counts.total >= self.total || from_ip >= self.per_address {
            return None;
        }
        counts.total += 1;
        *counts.by_ip.entry(ip).or_insert(0) += 1;
        Some(Slot {
            ip,
            counts: self.counts.clone(),
        })
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut counts = self.counts.lock().unwrap_or_else(|e| e.into_inner());
        counts.total = counts.total.saturating_sub(1);
        if let Some(n) = counts.by_ip.get_mut(&self.ip) {
            *n -= 1;
            if *n == 0 {
                counts.by_ip.remove(&self.ip);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn ip(last: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(192, 0, 2, last))
    }

    #[test]
    fn one_address_gets_only_its_share() {
        let budget = ConnBudget::new(2, 10);
        let _a = budget.admit(ip(1)).unwrap();
        let b = budget.admit(ip(1)).unwrap();
        assert!(
            budget.admit(ip(1)).is_none(),
            "a third from the same address"
        );
        assert!(
            budget.admit(ip(2)).is_some(),
            "another address still gets in"
        );
        drop(b);
        assert!(
            budget.admit(ip(1)).is_some(),
            "a closed connection frees its slot"
        );
    }

    #[test]
    fn the_server_as_a_whole_has_a_limit() {
        let budget = ConnBudget::new(10, 3);
        let slots: Vec<_> = (1..=3).map(|n| budget.admit(ip(n)).unwrap()).collect();
        assert!(budget.admit(ip(9)).is_none());
        drop(slots);
        assert!(budget.admit(ip(9)).is_some());
    }
}
