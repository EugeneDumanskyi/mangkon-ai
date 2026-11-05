//! A run's identity and states (domain §5.1 `AI_RUN`, ADR-0005), and the "Experimental" flag
//! (US-10.3).

/// What makes two runs "the same model": `local:<id>`, or `endpoint:<id>|<model>|<origin>`. The
/// gate reads an endpoint's config again on every call, so an edit mid-run would otherwise mix
/// two models' output silently; the lane compares this before every item.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct RunIdentity(String);

impl RunIdentity {
    pub fn local(model_id: &str) -> RunIdentity {
        RunIdentity(format!("local:{model_id}"))
    }

    pub fn endpoint(id: &str, model: &str, origin: &str) -> RunIdentity {
        RunIdentity(format!("endpoint:{id}|{model}|{origin}"))
    }

    /// As stored in `ai_run.model_identity`.
    pub fn from_stored(s: &str) -> RunIdentity {
        RunIdentity(s.to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_local(&self) -> bool {
        self.0.starts_with("local:")
    }
}

/// `active` is what the screens show, `pending` a re-run nobody sees yet, `superseded` an old
/// run whose mentions are gone (INV-AI-04). A cancelled re-run's row is deleted.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum RunState {
    Pending,
    Active,
    Superseded,
}

impl RunState {
    pub fn as_str(&self) -> &'static str {
        match self {
            RunState::Pending => "pending",
            RunState::Active => "active",
            RunState::Superseded => "superseded",
        }
    }

    pub fn from_name(name: &str) -> Option<RunState> {
        [RunState::Pending, RunState::Active, RunState::Superseded]
            .into_iter()
            .find(|s| s.as_str() == name)
    }
}

/// Whether the flag's value turns the label on: anything but `0`, `false` or `off`, and unset.
pub fn experimental_from(flag: Option<&str>) -> bool {
    !matches!(flag, Some("0" | "false" | "off"))
}

/// US-10.3 AC1: all LLM features show "Experimental"; `MK_AI_EXPERIMENTAL=0` at build time turns
/// the label off without a code change. On by default (M1 decision, roadmap §4 #6).
pub fn experimental() -> bool {
    experimental_from(option_env!("MK_AI_EXPERIMENTAL"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// US-10.3 AC1: on unless the flag says otherwise.
    #[test]
    fn us_10_3_ac1_flag() {
        assert!(experimental_from(None));
        assert!(experimental_from(Some("1")));
        for off in ["0", "false", "off"] {
            assert!(!experimental_from(Some(off)), "{off}");
        }
    }

    #[test]
    fn identity_says_what_changes_it() {
        let a = RunIdentity::endpoint("e1", "m", "https://h:443");
        assert_ne!(a, RunIdentity::endpoint("e1", "m2", "https://h:443"));
        assert_ne!(a, RunIdentity::endpoint("e1", "m", "https://other:443"));
        assert!(!a.is_local());
        assert!(RunIdentity::local("x").is_local());
        assert_eq!(RunIdentity::from_stored(a.as_str()), a);
    }

    #[test]
    fn run_state_names_round_trip() {
        for s in [RunState::Pending, RunState::Active, RunState::Superseded] {
            assert_eq!(RunState::from_name(s.as_str()), Some(s));
        }
        assert_eq!(RunState::from_name("x"), None);
    }
}
