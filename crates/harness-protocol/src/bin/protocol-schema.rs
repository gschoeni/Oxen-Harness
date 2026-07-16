//! Print the protocol's JSON Schema to stdout — the machine-readable spec a
//! client generator (e.g. `json-schema-to-typescript`) consumes:
//!
//! ```sh
//! cargo run -p harness-protocol --bin protocol-schema > docs/protocol-schema.json
//! ```

use harness_protocol::{
    ApprovalAnswer, ProtocolEvent, QuestionAnswer, SessionInfo, SessionView, TurnRequest,
    TurnResponse,
};

fn main() {
    let schema = serde_json::json!({
        "$comment": "oxen-harness wire protocol: the event stream plus command DTOs",
        "event": schemars::schema_for!(ProtocolEvent),
        "dtos": {
            "SessionInfo": schemars::schema_for!(SessionInfo),
            "SessionView": schemars::schema_for!(SessionView),
            "TurnRequest": schemars::schema_for!(TurnRequest),
            "TurnResponse": schemars::schema_for!(TurnResponse),
            "QuestionAnswer": schemars::schema_for!(QuestionAnswer),
            "ApprovalAnswer": schemars::schema_for!(ApprovalAnswer),
        },
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&schema).expect("schema serializes")
    );
}
