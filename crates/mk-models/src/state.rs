//! The model download state machine (domain §7.7) and its start guard (INV-MM-02, INV-MM-03).
//! Pure: no I/O.

use serde::Serialize;

use crate::GB;
use crate::hardware::{TOO_WEAK, gb};

/// Free space kept on top of what is left to download (US-09.4 AC1, INV-MM-02, ASSUMPTION
/// 2 GB).
pub const DISK_HEADROOM: u64 = 2 * GB;

/// US-09.4 AC4.
pub const CORRUPTED: &str = "Download corrupted. Please retry.";

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum DownloadState {
    Available,
    Downloading,
    Paused,
    Interrupted,
    Verifying,
    Installed,
}

/// What happens to a download.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// The user starts (guard passed).
    Start,
    Pause,
    Resume,
    /// Network lost, disk full, app quit.
    Interrupt,
    /// All bytes received.
    Received,
    ChecksumMatch,
    ChecksumMismatch,
    /// The partial files are deleted.
    Cancel,
    /// The installed model is deleted.
    Delete,
}

impl DownloadState {
    pub fn as_str(self) -> &'static str {
        match self {
            DownloadState::Available => "available",
            DownloadState::Downloading => "downloading",
            DownloadState::Paused => "paused",
            DownloadState::Interrupted => "interrupted",
            DownloadState::Verifying => "verifying",
            DownloadState::Installed => "installed",
        }
    }

    pub fn parse(s: &str) -> Option<DownloadState> {
        Some(match s {
            "available" => DownloadState::Available,
            "downloading" => DownloadState::Downloading,
            "paused" => DownloadState::Paused,
            "interrupted" => DownloadState::Interrupted,
            "verifying" => DownloadState::Verifying,
            "installed" => DownloadState::Installed,
            _ => return None,
        })
    }

    /// The next state, or `None` if `event` is not allowed in this state (domain §7.7).
    pub fn transition(self, event: Event) -> Option<DownloadState> {
        use DownloadState::*;
        use Event::*;
        Some(match (self, event) {
            (Available, Start) => Downloading,
            (Downloading, Pause) => Paused,
            (Paused, Resume) => Downloading,
            (Downloading, Interrupt) => Interrupted,
            (Interrupted, Resume) => Downloading,
            (Downloading, Received) => Verifying,
            (Verifying, ChecksumMatch) => Installed,
            (Verifying, ChecksumMismatch) => Available,
            // A failure while verifying (disk read error, app quit) is an interruption: the
            // file is kept and checked again on resume.
            (Verifying, Interrupt) => Interrupted,
            (Downloading | Paused | Interrupted, Cancel) => Available,
            (Installed, Delete) => Available,
            _ => return None,
        })
    }

    /// The state a stored row is in after the app restarts: a download or check that was
    /// running when the app quit is Interrupted (domain §7.7 "app quit").
    pub fn after_restart(self) -> DownloadState {
        match self {
            DownloadState::Downloading | DownloadState::Verifying => DownloadState::Interrupted,
            s => s,
        }
    }

    /// Whether files of a partial download are on disk.
    pub fn has_partial(self) -> bool {
        matches!(
            self,
            DownloadState::Downloading
                | DownloadState::Paused
                | DownloadState::Interrupted
                | DownloadState::Verifying
        )
    }
}

/// Why a download may not start, in plain words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// INV-MM-03, US-09.2 AC1.
    Hardware,
    /// INV-MM-02, US-09.4 AC1: `needed` bytes free, `free` available.
    Disk { needed: u64, free: u64 },
    /// The free space could not be read.
    DiskUnknown,
}

impl Refusal {
    pub fn code(&self) -> &'static str {
        match self {
            Refusal::Hardware => "below_minimum",
            Refusal::Disk { .. } | Refusal::DiskUnknown => "disk_full",
        }
    }

    pub fn message(&self) -> String {
        match self {
            Refusal::Hardware => TOO_WEAK.to_string(),
            Refusal::Disk { needed, free } => format!(
                "Not enough free disk space: this download needs {} GB free, {} GB is free.",
                gb(*needed),
                gb(*free)
            ),
            Refusal::DiskUnknown => {
                "The free disk space could not be read, so the download did not start.".into()
            }
        }
    }
}

/// Free disk a download with `remaining` bytes left needs (INV-MM-02).
pub fn disk_needed(remaining: u64) -> u64 {
    remaining.saturating_add(DISK_HEADROOM)
}

/// The start guard (domain §7.7): the hardware allows local models (INV-MM-03: GPU and memory
/// meet the minimum, or the override is on) and the disk has what is left to download plus
/// [`DISK_HEADROOM`] (INV-MM-02; ASSUMPTION: remaining bytes, so a resume near the end is not
/// refused for space the partial file already holds). The 30 GB of US-09.1 AC2 is a note in
/// the hardware check, not a condition here (decided 2026-11-04).
pub fn can_start(
    hardware_meets: bool,
    override_on: bool,
    free: Option<u64>,
    remaining: u64,
) -> Result<(), Refusal> {
    if !hardware_meets && !override_on {
        return Err(Refusal::Hardware);
    }
    let Some(free) = free else {
        return Err(Refusal::DiskUnknown);
    };
    let needed = disk_needed(remaining);
    if free < needed {
        return Err(Refusal::Disk { needed, free });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::DownloadState::*;
    use super::Event::*;
    use super::*;

    #[test]
    fn domain_7_7_transitions() {
        let path = [
            (Available, Start, Downloading),
            (Downloading, Pause, Paused),
            (Paused, Resume, Downloading),
            (Downloading, Interrupt, Interrupted),
            (Interrupted, Resume, Downloading),
            (Downloading, Received, Verifying),
            (Verifying, ChecksumMatch, Installed),
            (Installed, Delete, Available),
            (Verifying, ChecksumMismatch, Available),
            (Paused, Cancel, Available),
            (Interrupted, Cancel, Available),
            (Downloading, Cancel, Available),
        ];
        for (from, ev, to) in path {
            assert_eq!(from.transition(ev), Some(to), "{from:?} --{ev:?}-->");
        }
        // INV-MM-01: nothing reaches Installed but a matching checksum.
        for s in [Available, Downloading, Paused, Interrupted, Installed] {
            for ev in [
                Start,
                Pause,
                Resume,
                Interrupt,
                Received,
                ChecksumMatch,
                ChecksumMismatch,
                Cancel,
                Delete,
            ] {
                assert_ne!(s.transition(ev), Some(Installed), "{s:?} {ev:?}");
            }
        }
        assert_eq!(Installed.transition(Start), None);
        assert_eq!(Available.transition(Cancel), None);
    }

    #[test]
    fn app_quit_leaves_interrupted() {
        assert_eq!(Downloading.after_restart(), Interrupted);
        assert_eq!(Verifying.after_restart(), Interrupted);
        assert_eq!(Paused.after_restart(), Paused);
        assert_eq!(Installed.after_restart(), Installed);
        for s in [
            Available,
            Downloading,
            Paused,
            Interrupted,
            Verifying,
            Installed,
        ] {
            assert_eq!(DownloadState::parse(s.as_str()), Some(s));
        }
    }

    #[test]
    fn inv_mm_02_03_start_guard() {
        assert_eq!(
            can_start(false, false, Some(100 * GB), GB),
            Err(Refusal::Hardware)
        );
        assert_eq!(can_start(false, true, Some(100 * GB), GB), Ok(()));
        let r = can_start(false, true, Some(3 * GB), 2 * GB).unwrap_err();
        assert_eq!(
            r,
            Refusal::Disk {
                needed: 4 * GB,
                free: 3 * GB
            }
        );
        assert_eq!(
            r.message(),
            "Not enough free disk space: this download needs 4 GB free, 3 GB is free."
        );
        assert_eq!(can_start(false, true, Some(4 * GB), 2 * GB), Ok(()));
        // No 30 GB floor (decided 2026-11-04): a small download fits a nearly full disk.
        assert_eq!(can_start(true, false, Some(3 * GB), GB), Ok(()));
        assert_eq!(
            can_start(true, false, Some(40 * GB), 39 * GB),
            Err(Refusal::Disk {
                needed: 41 * GB,
                free: 40 * GB
            })
        );
        assert_eq!(can_start(true, false, None, 1), Err(Refusal::DiskUnknown));
    }
}
