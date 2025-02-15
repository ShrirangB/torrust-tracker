//! Axum [`handlers`](axum#handlers) for the `announce` requests.
//!
//! Refer to [HTTP server](crate::servers::http) for more information about the
//! `scrape` request.
//!
//! The handlers perform the authentication and authorization of the request,
//! and resolve the client IP address.
use std::sync::Arc;

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use log::debug;

use crate::core::auth::Key;
use crate::core::{ScrapeData, Tracker};
use crate::servers::http::v1::extractors::authentication_key::Extract as ExtractKey;
use crate::servers::http::v1::extractors::client_ip_sources::Extract as ExtractClientIpSources;
use crate::servers::http::v1::extractors::scrape_request::ExtractRequest;
use crate::servers::http::v1::requests::scrape::Scrape;
use crate::servers::http::v1::services::peer_ip_resolver::{self, ClientIpSources};
use crate::servers::http::v1::{responses, services};

/// It handles the `scrape` request when the HTTP tracker is configured
/// to run in `public` mode.
#[allow(clippy::unused_async)]
pub async fn handle_without_key(
    State(tracker): State<Arc<Tracker>>,
    ExtractRequest(scrape_request): ExtractRequest,
    ExtractClientIpSources(client_ip_sources): ExtractClientIpSources,
) -> Response {
    debug!("http scrape request: {:#?}", &scrape_request);

    handle(&tracker, &scrape_request, &client_ip_sources, None).await
}

/// It handles the `scrape` request when the HTTP tracker is configured
/// to run in `private` or `private_listed` mode.
///
/// In this case, the authentication `key` parameter is required.
#[allow(clippy::unused_async)]
pub async fn handle_with_key(
    State(tracker): State<Arc<Tracker>>,
    ExtractRequest(scrape_request): ExtractRequest,
    ExtractClientIpSources(client_ip_sources): ExtractClientIpSources,
    ExtractKey(key): ExtractKey,
) -> Response {
    debug!("http scrape request: {:#?}", &scrape_request);

    handle(&tracker, &scrape_request, &client_ip_sources, Some(key)).await
}

async fn handle(
    tracker: &Arc<Tracker>,
    scrape_request: &Scrape,
    client_ip_sources: &ClientIpSources,
    maybe_key: Option<Key>,
) -> Response {
    let scrape_data = match handle_scrape(tracker, scrape_request, client_ip_sources, maybe_key).await {
        Ok(scrape_data) => scrape_data,
        Err(error) => return error.into_response(),
    };
    build_response(scrape_data)
}

/* code-review: authentication, authorization and peer IP resolution could be moved
   from the handler (Axum) layer into the app layer `services::announce::invoke`.
   That would make the handler even simpler and the code more reusable and decoupled from Axum.
   See https://github.com/torrust/torrust-tracker/discussions/240.
*/

async fn handle_scrape(
    tracker: &Arc<Tracker>,
    scrape_request: &Scrape,
    client_ip_sources: &ClientIpSources,
    maybe_key: Option<Key>,
) -> Result<ScrapeData, responses::error::Error> {
    // Authentication
    let return_real_scrape_data = if tracker.requires_authentication() {
        match maybe_key {
            Some(key) => match tracker.authenticate(&key).await {
                Ok(()) => true,
                Err(_error) => false,
            },
            None => false,
        }
    } else {
        true
    };

    // Authorization for scrape requests is handled at the `Tracker` level
    // for each torrent.

    let peer_ip = match peer_ip_resolver::invoke(tracker.config.on_reverse_proxy, client_ip_sources) {
        Ok(peer_ip) => peer_ip,
        Err(error) => return Err(responses::error::Error::from(error)),
    };

    if return_real_scrape_data {
        Ok(services::scrape::invoke(tracker, &scrape_request.info_hashes, &peer_ip).await)
    } else {
        Ok(services::scrape::fake(tracker, &scrape_request.info_hashes, &peer_ip).await)
    }
}

fn build_response(scrape_data: ScrapeData) -> Response {
    responses::scrape::Bencoded::from(scrape_data).into_response()
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;
    use std::str::FromStr;

    use torrust_tracker_test_helpers::configuration;

    use crate::core::services::tracker_factory;
    use crate::core::Tracker;
    use crate::servers::http::v1::requests::scrape::Scrape;
    use crate::servers::http::v1::responses;
    use crate::servers::http::v1::services::peer_ip_resolver::ClientIpSources;
    use crate::shared::bit_torrent::info_hash::InfoHash;

    fn private_tracker() -> Tracker {
        tracker_factory(configuration::ephemeral_mode_private().into())
    }

    fn whitelisted_tracker() -> Tracker {
        tracker_factory(configuration::ephemeral_mode_whitelisted().into())
    }

    fn tracker_on_reverse_proxy() -> Tracker {
        tracker_factory(configuration::ephemeral_with_reverse_proxy().into())
    }

    fn tracker_not_on_reverse_proxy() -> Tracker {
        tracker_factory(configuration::ephemeral_without_reverse_proxy().into())
    }

    fn sample_scrape_request() -> Scrape {
        Scrape {
            info_hashes: vec!["3b245504cf5f11bbdbe1201cea6a6bf45aee1bc0".parse::<InfoHash>().unwrap()],
        }
    }

    fn sample_client_ip_sources() -> ClientIpSources {
        ClientIpSources {
            right_most_x_forwarded_for: Some(IpAddr::from_str("203.0.113.195").unwrap()),
            connection_info_ip: Some(IpAddr::from_str("203.0.113.196").unwrap()),
        }
    }

    fn assert_error_response(error: &responses::error::Error, error_message: &str) {
        assert!(
            error.failure_reason.contains(error_message),
            "Error response does not contain message: '{error_message}'. Error: {error:?}"
        );
    }

    mod with_tracker_in_private_mode {
        use std::str::FromStr;
        use std::sync::Arc;

        use super::{private_tracker, sample_client_ip_sources, sample_scrape_request};
        use crate::core::{auth, ScrapeData};
        use crate::servers::http::v1::handlers::scrape::handle_scrape;

        #[tokio::test]
        async fn it_should_return_zeroed_swarm_metadata_when_the_authentication_key_is_missing() {
            let tracker = Arc::new(private_tracker());

            let scrape_request = sample_scrape_request();
            let maybe_key = None;

            let scrape_data = handle_scrape(&tracker, &scrape_request, &sample_client_ip_sources(), maybe_key)
                .await
                .unwrap();

            let expected_scrape_data = ScrapeData::zeroed(&scrape_request.info_hashes);

            assert_eq!(scrape_data, expected_scrape_data);
        }

        #[tokio::test]
        async fn it_should_return_zeroed_swarm_metadata_when_the_authentication_key_is_invalid() {
            let tracker = Arc::new(private_tracker());

            let scrape_request = sample_scrape_request();
            let unregistered_key = auth::Key::from_str("YZSl4lMZupRuOpSRC3krIKR5BPB14nrJ").unwrap();
            let maybe_key = Some(unregistered_key);

            let scrape_data = handle_scrape(&tracker, &scrape_request, &sample_client_ip_sources(), maybe_key)
                .await
                .unwrap();

            let expected_scrape_data = ScrapeData::zeroed(&scrape_request.info_hashes);

            assert_eq!(scrape_data, expected_scrape_data);
        }
    }

    mod with_tracker_in_listed_mode {

        use std::sync::Arc;

        use super::{sample_client_ip_sources, sample_scrape_request, whitelisted_tracker};
        use crate::core::ScrapeData;
        use crate::servers::http::v1::handlers::scrape::handle_scrape;

        #[tokio::test]
        async fn it_should_return_zeroed_swarm_metadata_when_the_torrent_is_not_whitelisted() {
            let tracker = Arc::new(whitelisted_tracker());

            let scrape_request = sample_scrape_request();

            let scrape_data = handle_scrape(&tracker, &scrape_request, &sample_client_ip_sources(), None)
                .await
                .unwrap();

            let expected_scrape_data = ScrapeData::zeroed(&scrape_request.info_hashes);

            assert_eq!(scrape_data, expected_scrape_data);
        }
    }

    mod with_tracker_on_reverse_proxy {
        use std::sync::Arc;

        use super::{sample_scrape_request, tracker_on_reverse_proxy};
        use crate::servers::http::v1::handlers::scrape::handle_scrape;
        use crate::servers::http::v1::handlers::scrape::tests::assert_error_response;
        use crate::servers::http::v1::services::peer_ip_resolver::ClientIpSources;

        #[tokio::test]
        async fn it_should_fail_when_the_right_most_x_forwarded_for_header_ip_is_not_available() {
            let tracker = Arc::new(tracker_on_reverse_proxy());

            let client_ip_sources = ClientIpSources {
                right_most_x_forwarded_for: None,
                connection_info_ip: None,
            };

            let response = handle_scrape(&tracker, &sample_scrape_request(), &client_ip_sources, None)
                .await
                .unwrap_err();

            assert_error_response(
                &response,
                "Error resolving peer IP: missing or invalid the right most X-Forwarded-For IP",
            );
        }
    }

    mod with_tracker_not_on_reverse_proxy {
        use std::sync::Arc;

        use super::{sample_scrape_request, tracker_not_on_reverse_proxy};
        use crate::servers::http::v1::handlers::scrape::handle_scrape;
        use crate::servers::http::v1::handlers::scrape::tests::assert_error_response;
        use crate::servers::http::v1::services::peer_ip_resolver::ClientIpSources;

        #[tokio::test]
        async fn it_should_fail_when_the_client_ip_from_the_connection_info_is_not_available() {
            let tracker = Arc::new(tracker_not_on_reverse_proxy());

            let client_ip_sources = ClientIpSources {
                right_most_x_forwarded_for: None,
                connection_info_ip: None,
            };

            let response = handle_scrape(&tracker, &sample_scrape_request(), &client_ip_sources, None)
                .await
                .unwrap_err();

            assert_error_response(
                &response,
                "Error resolving peer IP: cannot get the client IP from the connection info",
            );
        }
    }
}
