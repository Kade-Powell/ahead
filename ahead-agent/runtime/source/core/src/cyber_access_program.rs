use ahead_model_auth::CodexAuth;
use codex_api::AccessPrograms;
use codex_protocol::turn_input::CyberAccessProgram;

pub(crate) fn for_auth(
    auth: Option<&CodexAuth>,
    program: Option<CyberAccessProgram>,
) -> Option<AccessPrograms> {
    program
        .filter(|_| auth.is_some_and(CodexAuth::is_chatgpt_auth))
        .map(AccessPrograms::from)
}
