//! Test faults (M2c acceptance criterion 2, spike S2b): a file whose first line is
//! `mk-test-fault: memory-bomb`, `endless-loop`, `crash` or `panic` makes the worker misbehave on purpose,
//! so the supervisor's limits can be tested on every OS. The worker honours the marker only when
//! the core sets [`Settings::test_faults`](crate::Settings); otherwise the file is plain text.

use std::hint::black_box;

pub const MARKER: &str = "mk-test-fault: ";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fault {
    /// Allocates and touches memory until it is stopped.
    MemoryBomb,
    /// Spins forever.
    EndlessLoop,
    /// Aborts the process.
    Crash,
    /// Panics where the parser runs.
    Panic,
}

impl Fault {
    pub fn as_str(&self) -> &'static str {
        match self {
            Fault::MemoryBomb => "memory-bomb",
            Fault::EndlessLoop => "endless-loop",
            Fault::Crash => "crash",
            Fault::Panic => "panic",
        }
    }

    /// The fault named on the first line of `bytes`, if any.
    pub fn in_file(bytes: &[u8]) -> Option<Fault> {
        let first = bytes.split(|b| *b == b'\n').next()?;
        let name = std::str::from_utf8(first).ok()?.trim_end_matches('\r');
        let name = name.strip_prefix(MARKER)?.trim();
        [
            Fault::MemoryBomb,
            Fault::EndlessLoop,
            Fault::Crash,
            Fault::Panic,
        ]
        .into_iter()
        .find(|f| f.as_str() == name)
    }

    /// Runs the fault. Never returns.
    pub fn run(self) -> ! {
        match self {
            Fault::MemoryBomb => {
                // 64 MiB blocks, every page touched so it counts as resident.
                let mut blocks: Vec<Vec<u8>> = Vec::new();
                loop {
                    let mut block = vec![0u8; 64 << 20];
                    for i in (0..block.len()).step_by(4096) {
                        block[i] = 1;
                    }
                    blocks.push(black_box(block));
                }
            }
            Fault::EndlessLoop => {
                let mut n = 0u64;
                loop {
                    n = black_box(n.wrapping_add(1));
                    std::hint::spin_loop();
                }
            }
            Fault::Crash => std::process::abort(),
            Fault::Panic => panic!("test fault: panic"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markers_on_the_first_line_only() {
        assert_eq!(
            Fault::in_file(b"mk-test-fault: memory-bomb\nmore"),
            Some(Fault::MemoryBomb)
        );
        assert_eq!(
            Fault::in_file(b"mk-test-fault: endless-loop\r\n"),
            Some(Fault::EndlessLoop)
        );
        assert_eq!(Fault::in_file(b"mk-test-fault: crash"), Some(Fault::Crash));
        assert_eq!(Fault::in_file(b"mk-test-fault: panic"), Some(Fault::Panic));
        assert_eq!(Fault::in_file(b"hello\nmk-test-fault: crash"), None);
        assert_eq!(Fault::in_file(b"mk-test-fault: other"), None);
        assert_eq!(Fault::in_file(b""), None);
    }
}
