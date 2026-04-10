use crate::AppState;
use axum::{
    extract::{ConnectInfo, Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::net::SocketAddr;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/protocol/challenge/new", get(new_challenge))
        .route("/protocol/challenge/:challenge", get(get_solution))
        .route(
            "/protocol/challenge/:challenge/:solution",
            get(submit_solution),
        )
        .route("/protocol/challenge", post(store_challenge))
}

#[derive(serde::Deserialize)]
struct NewChallengeParams {
    miner_uid: Option<String>,
}

async fn new_challenge(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(params): Query<NewChallengeParams>,
) -> Json<Value> {
    if !crate::networking::network::is_local_request(&addr) {
        return Json(json!({"error": "Request not from localhost"}));
    }

    let challenge = uuid::Uuid::new_v4().to_string();
    let challenge_url = format!(
        "{}/protocol/challenge/{}",
        state.config.base_url(),
        challenge
    );
    Json(json!({
        "challenge": challenge,
        "challenge_url": challenge_url,
        "miner_uid": params.miner_uid,
    }))
}

async fn get_solution(State(state): State<AppState>, Path(challenge): Path<String>) -> Json<Value> {
    match crate::db::challenge_response::read_challenge_solution(&state.db, &challenge) {
        Ok(Some(solution)) => Json(json!({"challenge": challenge, "solution": solution})),
        Ok(None) => Json(json!({"error": "Challenge not found"})),
        Err(e) => Json(json!({"error": e.to_string()})),
    }
}

async fn submit_solution(
    State(state): State<AppState>,
    Path((challenge, solution)): Path<(String, String)>,
) -> Json<Value> {
    match crate::db::challenge_response::write_challenge_solution_pair(
        &state.db, &challenge, &solution,
    ) {
        Ok(()) => Json(json!({"success": true})),
        Err(e) => Json(json!({"error": e.to_string()})),
    }
}

#[derive(serde::Deserialize)]
struct ChallengePayload {
    url: Option<String>,
    challenge: Option<String>,
    solution: Option<String>,
}

async fn store_challenge(
    State(state): State<AppState>,
    Json(payload): Json<ChallengePayload>,
) -> Json<Value> {
    if let (Some(challenge), Some(solution)) = (payload.challenge, payload.solution) {
        match crate::db::challenge_response::write_challenge_solution_pair(
            &state.db, &challenge, &solution,
        ) {
            Ok(()) => Json(json!({"success": true})),
            Err(e) => Json(json!({"error": e.to_string()})),
        }
    } else if let Some(url) = payload.url {
        // Forward challenge to the URL and get response
        match reqwest::get(&url).await {
            Ok(resp) => {
                let text = resp.text().await.unwrap_or_default();
                Json(json!({"response": text}))
            }
            Err(e) => Json(json!({"error": e.to_string()})),
        }
    } else {
        Json(json!({"error": "Missing challenge/solution or url"}))
    }
}
