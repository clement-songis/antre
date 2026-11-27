//! Trade an API key for a browser session, for clients that host the web UI
//! in a WebView (desktop and mobile shells).
//!
//! A WebView cannot add `Authorization: Bearer` to every request and to the
//! WebSocket, and the web UI authenticates with the session cookie. The
//! client mints a single-use ticket with its API key (`POST
//! /api/auth/session-ticket`, `operator.admin` scope, since a session has
//! full access), then navigates the WebView to `GET
//! /api/auth/session-ticket/redeem?ticket=…`, which sets the cookie and
//! redirects to the app.

use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::{
    Json,
    extract::{ConnectInfo, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};

use {
    super::{AuthState, blocked_response, localhost_cookie_domain, should_secure_cookie},
    crate::auth_middleware::{RequireAdmin, SESSION_COOKIE},
};

/// How long a ticket stays redeemable. Long enough for a WebView to load
/// the redeem URL, short enough that a leaked URL is useless soon after.
pub(crate) const TICKET_TTL: Duration = Duration::from_secs(60);
/// Unredeemed tickets kept at most; the oldest is dropped beyond that.
const MAX_PENDING_TICKETS: usize = 64;
/// Login-guard account for failed redemptions (rate limits guessing).
const TICKET_ACCOUNT: &str = "__session_ticket__";

/// Single-use tickets waiting to be redeemed, with their expiry.
#[derive(Clone, Default)]
pub struct SessionTickets(Arc<Mutex<HashMap<String, Instant>>>);

impl SessionTickets {
    /// A new random ticket, valid for [`TICKET_TTL`].
    pub(crate) fn issue(&self) -> String {
        self.issue_at(Instant::now())
    }

    fn issue_at(&self, now: Instant) -> String {
        let bytes: [u8; 32] = rand::random();
        let ticket: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        let mut pending = self.0.lock().unwrap_or_else(|e| e.into_inner());
        pending.retain(|_, expires| *expires > now);
        while pending.len() >= MAX_PENDING_TICKETS {
            let Some(oldest) = pending
                .iter()
                .min_by_key(|(_, expires)| **expires)
                .map(|(ticket, _)| ticket.clone())
            else {
                break;
            };
            pending.remove(&oldest);
        }
        pending.insert(ticket.clone(), now + TICKET_TTL);
        ticket
    }

    /// Consume `ticket`: true once, if it was issued and has not expired.
    pub(crate) fn redeem(&self, ticket: &str) -> bool {
        self.redeem_at(ticket, Instant::now())
    }

    fn redeem_at(&self, ticket: &str, now: Instant) -> bool {
        let mut pending = self.0.lock().unwrap_or_else(|e| e.into_inner());
        pending.remove(ticket).is_some_and(|expires| expires > now)
    }
}

pub(super) async fn issue_handler(
    State(state): State<AuthState>,
    RequireAdmin(_identity): RequireAdmin,
) -> Response {
    let ticket = state.session_tickets.issue();
    state
        .credential_store
        .audit_log("session_ticket_issued", None, None)
        .await;
    Json(serde_json::json!({
        "ticket": ticket,
        "expires_in": TICKET_TTL.as_secs(),
        "redeem_path": format!("/api/auth/session-ticket/redeem?ticket={ticket}"),
    }))
    .into_response()
}

#[derive(serde::Deserialize)]
pub(super) struct RedeemQuery {
    ticket: String,
}

pub(super) async fn redeem_handler(
    State(state): State<AuthState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: axum::http::HeaderMap,
    Query(query): Query<RedeemQuery>,
) -> Response {
    let behind_proxy = state.gateway_state.behind_proxy;
    let client_ip = crate::request_throttle::resolve_client_ip(&headers, addr, behind_proxy);
    if let Some(block) = state.login_guard.check(client_ip, TICKET_ACCOUNT) {
        return blocked_response(block);
    }
    let ip = client_ip.to_string();
    if !state.session_tickets.redeem(&query.ticket) {
        state.login_guard.record_failure(client_ip, TICKET_ACCOUNT);
        state
            .credential_store
            .audit_log("session_ticket_rejected", Some(&ip), None)
            .await;
        return (StatusCode::UNAUTHORIZED, "invalid or expired ticket").into_response();
    }
    state.login_guard.record_success(client_ip);
    let token = match state.credential_store.create_session().await {
        Ok(token) => token,
        Err(error) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("session error: {error}"),
            )
                .into_response();
        },
    };
    state
        .credential_store
        .audit_log("session_ticket_redeemed", Some(&ip), None)
        .await;
    let secure = should_secure_cookie(state.gateway_state.tls_active, behind_proxy, &headers);
    let domain_attr = localhost_cookie_domain(&headers, behind_proxy);
    let secure_attr = if secure {
        "; Secure"
    } else {
        ""
    };
    let cookie = format!(
        "{SESSION_COOKIE}={token}; HttpOnly; SameSite=Strict; Path=/; Max-Age=2592000{domain_attr}{secure_attr}"
    );
    (StatusCode::SEE_OTHER, [
        (header::SET_COOKIE, cookie),
        (header::LOCATION, "/".to_string()),
        (header::CACHE_CONTROL, "no-store".to_string()),
    ])
        .into_response()
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ticket_redeems_exactly_once() {
        let tickets = SessionTickets::default();
        let ticket = tickets.issue();
        assert_eq!(ticket.len(), 64);
        assert!(tickets.redeem(&ticket));
        assert!(!tickets.redeem(&ticket));
    }

    #[test]
    fn unknown_tickets_are_rejected() {
        let tickets = SessionTickets::default();
        tickets.issue();
        assert!(!tickets.redeem("not-a-ticket"));
        assert!(!tickets.redeem(""));
    }

    #[test]
    fn expired_tickets_are_rejected() {
        let tickets = SessionTickets::default();
        let now = Instant::now();
        let ticket = tickets.issue_at(now);
        assert!(!tickets.redeem_at(&ticket, now + TICKET_TTL + Duration::from_secs(1)));
    }

    #[test]
    fn tickets_are_unique() {
        let tickets = SessionTickets::default();
        assert_ne!(tickets.issue(), tickets.issue());
    }

    #[test]
    fn pending_tickets_are_bounded() {
        let tickets = SessionTickets::default();
        let start = Instant::now();
        let first = tickets.issue_at(start);
        for step in 1..=MAX_PENDING_TICKETS {
            tickets.issue_at(start + Duration::from_millis(step as u64));
        }
        let pending = tickets.0.lock().unwrap().len();
        assert!(pending <= MAX_PENDING_TICKETS);
        assert!(
            !tickets.redeem_at(&first, start),
            "the oldest ticket is dropped first"
        );
    }
}
