//! The HTTP client both applications make requests with.
//!
//! Blocking, because neither application has an async runtime: fetches happen
//! on worker threads that publish through a channel, and a frame never waits on
//! one.
//!
//! What is shared is the defaults, not the requests. Every caller names itself,
//! reads statuses rather than having them raised, follows a bounded number of
//! redirects and gives up after a bounded time -- and each of those was a bug
//! somewhere before it was a default here.

use std::time::Duration;

/// How long a request may take in total, connection and body included.
///
/// A whole request rather than a read timeout: a server that sends a byte a
/// second never trips a read timeout and never finishes either, and the thread
/// waiting on it is one an album cover or a presence update is queued behind.
const TIMEOUT: Duration = Duration::from_secs(15);

/// Redirects followed before giving up.
///
/// Three hops is more than any endpoint either application talks to needs --
/// the Cover Art Archive's own chain is the longest at two -- and the library's
/// default of ten is room for a redirect loop to spend.
const MAX_REDIRECTS: u32 = 5;

/// The shared configuration, for a caller with one more thing to say about it.
///
/// `https_only` is deliberately not set here. It belongs on almost every agent
/// and is [`agent`]'s whole difference from this, but a caller that points an
/// agent at a plaintext socket of its own -- a test proving the User-Agent
/// really goes out on the wire -- needs a way to say so.
pub fn builder(user_agent: &str) -> ureq::config::ConfigBuilder<ureq::typestate::AgentScope> {
    ureq::Agent::config_builder()
        .user_agent(user_agent)
        // Statuses are read rather than raised, because the body is what
        // distinguishes a busy server from a rate limit, and an error that has
        // already thrown the body away cannot tell them apart.
        .http_status_as_error(false)
        .max_redirects(MAX_REDIRECTS)
        .timeout_global(Some(TIMEOUT))
}

/// An agent that will only talk to https.
///
/// `user_agent` is not decoration: MusicBrainz answers 503 to a client that
/// does not name itself, and a header that silently failed to apply would look
/// exactly like the service being down. `concat!("<app>/", env!("CARGO_PKG_VERSION"))`
/// is what a caller passes.
///
/// Every endpoint either application talks to is https, and a redirect is the
/// one place that could quietly stop being true: without this, a compromised or
/// intercepted service can answer 302 to an http:// URL and the request goes
/// out again in the clear.
pub fn agent(user_agent: &str) -> ureq::Agent {
    builder(user_agent).https_only(true).build().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_the_ones_written_down() {
        let agent = agent("starkit-test/0.1");
        let config = agent.config();
        assert!(
            matches!(
                config.user_agent(),
                ureq::config::AutoHeaderValue::Provided(name) if **name == *"starkit-test/0.1"
            ),
            "the caller's name is not the one that would go out: {:?}",
            config.user_agent()
        );
        assert!(config.https_only(), "plaintext is not a default");
        assert!(!config.http_status_as_error());
        assert_eq!(config.max_redirects(), MAX_REDIRECTS);
        assert_eq!(config.timeouts().global, Some(TIMEOUT));
    }

    #[test]
    fn the_builder_leaves_the_https_rule_to_the_caller() {
        // The one thing a caller may need to say differently, and the reason
        // the builder is public at all.
        let agent: ureq::Agent = builder("starkit-test/0.1").https_only(false).build().into();
        assert!(!agent.config().https_only());
        // Everything else is still the shared default.
        assert_eq!(agent.config().max_redirects(), MAX_REDIRECTS);
    }
}
