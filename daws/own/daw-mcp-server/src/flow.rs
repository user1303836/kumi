//! The host's side of flow control. A bridge that names `creditBytes` in its welcome (the Renoise tool) sends only within
//! the credit the host grants, so that it never sends into a full socket: Renoise's GUI thread would block on it. The
//! host therefore has to know how many bytes can sit unread. It asks for a large receive buffer before it connects, reads
//! back what the system gave, and lets the bridge run that far ahead of its reading and no further: the bridge starts
//! with its floor, the host's first grant tops that up to the buffer, and from then on the host grants what it has read.
use daw_bridge::wire::CREDIT_FLOOR_BYTES;

/// The receive buffer the host asks for, in bytes.
pub const RECV_BUFFER_BYTES: u32 = 1024 * 1024;

/// How many bytes can wait unread in a receive buffer the system says holds `held`. Linux reports twice what was asked for
/// (half of the buffer goes to bookkeeping), so a buffer there holds half of what it reports. Never less than the
/// bridge's floor, which it has already been given, and never more than was asked for. `held` of 0 (the system would not
/// say) is the floor: no top-up.
pub fn window_from(held: usize) -> u64 {
    let usable = if cfg!(target_os = "linux") { held / 2 } else { held } as u64;
    if held == 0 {
        return CREDIT_FLOOR_BYTES;
    }
    usable.clamp(CREDIT_FLOOR_BYTES, RECV_BUFFER_BYTES as u64)
}

/// What the host has granted a bridge, and what it has read.
#[derive(Debug, Clone)]
pub struct Credit {
    window: u64,
    /// Bytes the bridge has been given in all, its floor included.
    granted: u64,
    /// Bytes the host has taken from the socket since the bridge started counting.
    read: u64,
    /// Bytes read and not yet granted back.
    owed: u64,
}

impl Credit {
    /// The state for a bridge whose welcome gave `floor`, and the host's first grant, which tops the bridge up to the
    /// window.
    pub fn new(floor: u64, window: u64) -> (Credit, Option<u64>) {
        let window = window.max(floor);
        let first = window - floor;
        let credit = Credit { window, granted: window, read: 0, owed: 0 };
        (credit, (first > 0).then_some(first))
    }

    /// The host has taken `bytes` from the socket (whole lines or not): the grant to send now, if it has come to a
    /// quarter of the window. A host that stops reading stops granting.
    pub fn read(&mut self, bytes: u64) -> Option<u64> {
        self.read += bytes;
        self.owed += bytes;
        if self.owed >= (self.window / 4).max(1) {
            let grant = std::mem::take(&mut self.owed);
            self.granted += grant;
            return Some(grant);
        }
        None
    }

    /// What the host lets run unread.
    pub fn window(&self) -> u64 {
        self.window
    }

    /// What the bridge may send and the host has not yet read: never over the window.
    pub fn unread(&self) -> u64 {
        self.granted.saturating_sub(self.read)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_grant_tops_the_floor_up_to_the_window() {
        let (credit, first) = Credit::new(64 * 1024, 1024 * 1024);
        assert_eq!(first, Some(1024 * 1024 - 64 * 1024));
        assert_eq!(credit.unread(), 1024 * 1024);
        // A window no bigger than the floor is given nothing more.
        let (small, none) = Credit::new(64 * 1024, 64 * 1024);
        assert_eq!((none, small.unread()), (None, 64 * 1024));
        let (smaller, none) = Credit::new(64 * 1024, 16 * 1024);
        assert_eq!((none, smaller.window()), (None, 64 * 1024), "the floor was given already");
    }

    #[test]
    fn what_is_read_is_granted_back_a_quarter_of_the_window_at_a_time_and_unread_stays_inside_it() {
        let (mut credit, _) = Credit::new(64 * 1024, 256 * 1024);
        let mut granted_back = 0;
        for read in [1u64, 1000, 60_000, 64 * 1024, 5, 200_000, 1] {
            if let Some(grant) = credit.read(read) {
                assert!(grant >= 64 * 1024, "{grant}: a quarter of 256 KiB");
                granted_back += grant;
            }
            assert!(credit.unread() <= credit.window(), "{} over {}", credit.unread(), credit.window());
        }
        let read = 1 + 1000 + 60_000 + 64 * 1024 + 5 + 200_000 + 1;
        assert!(granted_back <= read && read - granted_back < 64 * 1024, "granted back {granted_back} of {read}");
    }

    #[test]
    fn a_host_that_reads_nothing_grants_nothing() {
        let (mut credit, _) = Credit::new(64 * 1024, 256 * 1024);
        assert_eq!(credit.read(0), None);
        assert_eq!(credit.unread(), 256 * 1024);
    }

    #[test]
    fn the_window_comes_from_what_the_system_reports() {
        let linux = cfg!(target_os = "linux");
        // Asked for 1 MiB and got it (Linux reports twice what it will hold, up to its cap).
        let held = if linux { 2 * 1024 * 1024 } else { 1024 * 1024 };
        assert_eq!(window_from(held), 1024 * 1024);
        // A system that cut it: 416 KiB reported on Linux is 208 KiB of data.
        let cut = 416 * 1024;
        assert_eq!(window_from(cut), if linux { 208 * 1024 } else { 416 * 1024 });
        // Never under the floor, never over what was asked, and 0 means it would not say.
        assert_eq!(window_from(8 * 1024), CREDIT_FLOOR_BYTES);
        assert_eq!(window_from(64 * 1024 * 1024), RECV_BUFFER_BYTES as u64);
        assert_eq!(window_from(0), CREDIT_FLOOR_BYTES);
    }
}
