import sys

p = 'src/commands/delivery_controller.rs'
s = open(p, encoding='utf-8').read()

def rep(old, new, count=1):
    global s
    found = s.count(old)
    assert found == count, "expected %d, found %d for:\n%s" % (count, found, old[:120])
    s = s.replace(old, new)

# --- the helper ---
anchor = '''/// Single authority for the post-transcription delivery lifecycle.'''
assert anchor in s
helper = '''/// Build the review context payload.
///
/// The `llm_text` rule — "the LLM output is persisted only when the LLM
/// actually ran" — used to be spelled out at each of the three sites
/// that build a `ReviewData`. Changing the persisted shape meant three
/// edits, and a missed one compiled fine (the rule had no name and no
/// guard).
fn build_review_data(
    save_result: &Option<SaveResult>,
    raw_transcription: &str,
    llm_text: &str,
    llm_enabled: bool,
) -> Option<ReviewData> {
    save_result.as_ref().map(|sr| ReviewData {
        json_path: sr.json_path.clone(),
        raw_transcription: raw_transcription.to_string(),
        llm_text: if llm_enabled {
            Some(llm_text.to_string())
        } else {
            None
        },
    })
}

'''
s = s.replace(anchor, helper + anchor, 1)

# --- the three sites ---
rep(
    """            let review_data = save_result.map(|sr| ReviewData {
                json_path: sr.json_path,
                raw_transcription: transcription.clone(),
                llm_text: if policy.llm_enabled {
                    Some(final_text.clone())
                } else {
                    None
                },
            });""",
    """            let review_data = build_review_data(
                &save_result,
                &transcription,
                &final_text,
                policy.llm_enabled,
            );""",
    2,
)
rep(
    """        let review_data = save_result.map(|sr| ReviewData {
            json_path: sr.json_path,
            raw_transcription: transcription,
            llm_text: if policy.llm_enabled {
                Some(text.clone())
            } else {
                None
            },
        });""",
    """        let review_data = build_review_data(
            &save_result,
            &transcription,
            &text,
            policy.llm_enabled,
        );""",
    1,
)

open(p, 'w', encoding='utf-8').write(s)
sys.stderr.write("delivery_controller: build_review_data extracted\n")
