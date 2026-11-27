//! `POST /api/auth/session-ticket` + `GET /api/auth/session-ticket/redeem`:
//! a WebView shell trades an API key for a session cookie.

use super::*;

fn no_redirects() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

async fn issue_ticket(addr: SocketAddr, key: &str) -> reqwest::Response {
    no_redirects()
        .post(format!("http://{addr}/api/auth/session-ticket"))
        .header("Authorization", format!("Bearer {key}"))
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn admin_api_key_trades_a_ticket_for_a_session_cookie() {
    let (addr, store) = start_auth_server().await;
    set_initial_test_password(&store).await;
    let (_id, key) = store.create_api_key("shell", None).await.unwrap();

    let issued = issue_ticket(addr, &key).await;
    assert_eq!(issued.status(), 200);
    let body: serde_json::Value = issued.json().await.unwrap();
    let redeem_path = body["redeem_path"].as_str().unwrap().to_owned();
    assert_eq!(body["expires_in"], 60);

    let redeemed = no_redirects()
        .get(format!("http://{addr}{redeem_path}"))
        .send()
        .await
        .unwrap();
    assert_eq!(redeemed.status(), 303);
    assert_eq!(redeemed.headers()["location"], "/");
    let cookie = redeemed.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(cookie.contains("HttpOnly"), "{cookie}");
    let token = cookie
        .split(';')
        .next()
        .and_then(|pair| pair.split_once('='))
        .map(|(_, value)| value.to_owned())
        .unwrap();
    assert!(store.validate_session(&token).await.unwrap());

    let again = no_redirects()
        .get(format!("http://{addr}{redeem_path}"))
        .send()
        .await
        .unwrap();
    assert_eq!(again.status(), 401, "tickets are single-use");
}

#[tokio::test]
async fn non_admin_api_key_cannot_mint_a_session() {
    let (addr, store) = start_auth_server().await;
    set_initial_test_password(&store).await;
    let scopes = vec!["operator.read".to_owned()];
    let (_id, key) = store.create_api_key("reader", Some(&scopes)).await.unwrap();
    assert_eq!(issue_ticket(addr, &key).await.status(), 403);
}

#[tokio::test]
async fn missing_or_wrong_credentials_get_no_ticket() {
    let (addr, store) = start_auth_server().await;
    set_initial_test_password(&store).await;
    assert_eq!(issue_ticket(addr, "mk_not_a_key").await.status(), 401);
    let anonymous = no_redirects()
        .post(format!("http://{addr}/api/auth/session-ticket"))
        .send()
        .await
        .unwrap();
    assert_eq!(anonymous.status(), 401);
}

#[tokio::test]
async fn forged_tickets_are_rejected() {
    let (addr, store) = start_auth_server().await;
    set_initial_test_password(&store).await;
    let response = no_redirects()
        .get(format!(
            "http://{addr}/api/auth/session-ticket/redeem?ticket=deadbeef"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 401);
    assert!(response.headers().get("set-cookie").is_none());
}
