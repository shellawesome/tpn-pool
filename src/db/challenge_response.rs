use super::DbPool;
use anyhow::Result;
use chrono::Utc;

/// Write a challenge-solution pair (upsert).
pub fn write_challenge_solution_pair(pool: &DbPool, challenge: &str, solution: &str) -> Result<()> {
    let conn = pool.get()?;
    let now = Utc::now().timestamp_millis();
    conn.execute(
        "INSERT INTO challenge_solution (challenge, solution, updated)
         VALUES (?1, ?2, ?3)
         ON CONFLICT (challenge) DO UPDATE SET
            solution = excluded.solution,
            updated = excluded.updated",
        rusqlite::params![challenge, solution, now],
    )?;
    Ok(())
}

/// Read a solution by challenge key.
pub fn read_challenge_solution(pool: &DbPool, challenge: &str) -> Result<Option<String>> {
    let conn = pool.get()?;
    let solution = conn
        .query_row(
            "SELECT solution FROM challenge_solution WHERE challenge = ?1",
            rusqlite::params![challenge],
            |row| row.get::<_, String>(0),
        )
        .ok();
    Ok(solution)
}
